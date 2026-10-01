use super::*;
use std::{cell::RefCell, collections::BTreeMap};

tokio::task_local! {
    static PHASES: RefCell<BTreeMap<&'static str, f64>>;
}

// Task-local rather than thread-local: concurrent launches can migrate between
// executor threads. Outside a benchmark the guard collects nothing.
pub(super) struct Phase(&'static str, Instant);

impl Phase {
    pub(super) fn start(name: &'static str) -> Self {
        Self(name, Instant::now())
    }
}

impl Drop for Phase {
    fn drop(&mut self) {
        let elapsed = self.1.elapsed().as_secs_f64() * 1000.;
        let _ = PHASES.try_with(|phases| {
            *phases.borrow_mut().entry(self.0).or_default() += elapsed;
        });
    }
}

fn summarize(values: &[Option<f64>]) -> Value {
    let mut successful: Vec<_> = values.iter().flatten().copied().collect();
    successful.sort_by(f64::total_cmp);
    let percentile = |p: usize| {
        successful
            .get((successful.len() * p).div_ceil(100).saturating_sub(1))
            .copied()
    };
    json!({"successes":successful.len(),"failures":values.len()-successful.len(),"p50_ms":percentile(50),"p95_ms":percentile(95),"p99_ms":percentile(99)})
}

impl Runtime {
    pub async fn benchmark(
        &self,
        image: &Path,
        template: &str,
        samples: usize,
        concurrency: usize,
    ) -> Result<Value> {
        if !(1..=100).contains(&samples) || !(1..=4).contains(&concurrency) {
            return Err(Error::Invalid(
                "samples must be 1..100, concurrency 1..4".into(),
            ));
        }
        let prefix = format!("bench-{}", id()?);
        let mut groups = serde_json::Map::new();
        for mode in ["cold_boot", "template_restore"] {
            let mut observations = Vec::new();
            for offset in (0..samples).step_by(concurrency) {
                let mut workers = tokio::task::JoinSet::new();
                for index in offset..(offset + concurrency).min(samples) {
                    let runtime = Runtime {
                        root: self.root.clone(),
                        isolation: self.isolation.clone(),
                    };
                    let image = image.to_owned();
                    let template = template.to_owned();
                    let name = format!("{prefix}-{index}");
                    workers.spawn(async move {
                        let start = Instant::now();
                        let (launched, phases) = PHASES.scope(RefCell::new(BTreeMap::new()), async {
                            let launched = if mode == "cold_boot" { runtime.create(&image, &name, 256, 1, false).await } else { runtime.clone_template(&template, &name).await };
                            (launched, PHASES.with(|phases| phases.take()))
                        }).await;
                        let launch_ms = start.elapsed().as_secs_f64()*1000.;
                        let mut copy_method = None;
                        let result = async {
                            let record = launched?;
                            copy_method = Some(record.disk_copy);
                            let result = runtime.exec(&record.id, ExecRequest {argv:vec!["/bin/printf".into(),"boxd-ready".into()],cwd:None,env:Default::default(),timeout_ms:5000}).await?;
                            if result.stdout != b"boxd-ready" || result.exit_code != Some(0) || result.timed_out {
                                return Err(Error::Invalid("first command returned unexpected output or exit code".into()));
                            }
                            Ok(())
                        }.await;
                        let elapsed = start.elapsed().as_secs_f64()*1000.;
                        json!({"index":index,"launch_ms":launch_ms,"phases_ms":phases,"first_exec_ms":elapsed-launch_ms,"total_ms":result.as_ref().ok().map(|_|elapsed),"attempt_ms":elapsed,"disk_copy":copy_method,"error":result.err().map(|e:Error|e.to_string())})
                    });
                }
                while let Some(result) = workers.join_next().await {
                    observations
                        .push(result.map_err(|e| {
                            Error::Invalid(format!("benchmark worker failed: {e}"))
                        })?);
                }
                // Outside the measured region; only this invocation's unique
                // names may be cleaned up, including failed launch reservations.
                for record in self
                    .records()?
                    .into_iter()
                    .filter(|r| r.name.starts_with(&prefix))
                {
                    self.delete(&record.id).await?;
                }
            }
            observations.sort_by_key(|v| v["index"].as_u64());
            let times: Vec<_> = observations
                .iter()
                .map(|v| v["total_ms"].as_f64())
                .collect();
            groups.insert(
                mode.into(),
                json!({"summary":summarize(&times),"samples":observations}),
            );
        }
        Ok(
            json!({"schema_version":1,"samples_per_mode":samples,"concurrency":concurrency,"cache_condition":"uncontrolled OS page cache; all checksums verified on every launch; no cache eviction", "build_profile":if cfg!(debug_assertions) {"debug"} else {"release"},"clock_scope":"runtime request through first successful guest command; excludes benchmark startup and cleanup", "host":host::fingerprint()?,"versions":host::check(),"image":image,"template":template,"results":groups}),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn nearest_rank_counts_failures_without_inventing_latencies() {
        let report = summarize(&[Some(1.), Some(2.), Some(3.), Some(4.), Some(100.), None]);
        assert_eq!(report["p50_ms"], 3.);
        assert_eq!(report["p95_ms"], 100.);
        assert_eq!(report["failures"], 1);
        assert_eq!(report["successes"], 5);
        let empty = summarize(&[None]);
        assert_eq!(empty["p50_ms"], json!(null));
        assert_eq!(empty["failures"], 1);
    }
}
