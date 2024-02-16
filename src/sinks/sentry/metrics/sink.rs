use async_trait::async_trait;
use futures::{stream::BoxStream, StreamExt};
use futures_util::future::ready;
use sentry::metrics::{Metric, MetricBuilder};
use std::time::{Duration, UNIX_EPOCH};

use crate::{
    event::{Event, Metric as SourceMetric, MetricValue},
    sinks::util::StreamSink,
};

pub struct SentryMetricsSink {
    pub dsn: String,
}

fn finish_metric(source_metric: &SourceMetric, mut sentry_metric: MetricBuilder) {
    if let Some(tags) = source_metric.tags() {
        for (k, v) in tags.iter_all() {
            if let Some(v) = v {
                sentry_metric = sentry_metric.with_tag(k.to_owned(), v.to_owned());
            }
        }
    }

    if let Some(meta_object) = source_metric.metadata().value().as_object() {
        if let Some(unit) = meta_object.get("sentry_unit") {
            if let Some(unit) = unit.as_str() {
                sentry_metric = sentry_metric.with_unit(unit.into_owned());
            }
        }
    }

    // we need to carry forward the original timestamp as otherwise the SDK will use the current
    // time. otherwise this causes bad visual artifacts when backpressure happens and unnecessarily
    // relies on vector's system clock.
    if let Some(time) = source_metric.timestamp() {
        if let Ok(time) = u64::try_from(time.timestamp()) {
            let system_time = UNIX_EPOCH + Duration::from_secs(time);
            // XXX: why does the SDK need a SystemTime here?
            sentry_metric = sentry_metric.with_time(system_time);
        }
    }

    sentry_metric.send();
}

#[async_trait]
impl StreamSink<Event> for SentryMetricsSink {
    async fn run(mut self: Box<Self>, input: BoxStream<'_, Event>) -> Result<(), ()> {
        let mut input = input
            // filter out any non-metric events
            .filter_map(|event| ready(event.try_into_metric()));

        // https://github.com/getsentry/sentry-rust/issues/637
        let _guard = sentry::init(self.dsn);

        while let Some(source_metric) = input.next().await {
            let series = source_metric.series();
            let series_name = series.name();
            // in the case of datadog-agent being the source, series_name.namespace can be
            // system/docker/nginx
            // series_name.name is the rest of the metric name
            // without the namespace, we will get metrics like "cpu.usage" instead of
            // "docker.cpu.usage"
            let mut name = series_name.namespace.clone().unwrap_or_default();
            if !name.is_empty() {
                name.push('.');
            }

            name.push_str(&series_name.name);

            match source_metric.data().value() {
                MetricValue::Counter { value } => {
                    finish_metric(&source_metric, Metric::incr(name, *value));
                }
                MetricValue::Gauge { value } => {
                    finish_metric(&source_metric, Metric::gauge(name, *value));
                }
                MetricValue::Set { values } => {
                    for value in values {
                        // XXX: why not submit the entire set at once?
                        finish_metric(&source_metric, Metric::set(name.clone(), &value));
                    }
                }
                MetricValue::Distribution { samples, .. } => {
                    for sample in samples {
                        for _ in 0..sample.rate {
                            // XXX: sentry should allow me to submit value + count
                            finish_metric(
                                &source_metric,
                                Metric::distribution(name.clone(), sample.value),
                            );
                        }
                    }
                }
                _ => (),
            }
        }

        Ok(())
    }
}
