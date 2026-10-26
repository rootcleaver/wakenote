use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::time::Duration;

/// At most one hardware-volume call may remain in flight. Dropping a timed
/// out JoinHandle does not cancel Core Audio, so keep the gate until it exits.
#[derive(Clone, Default)]
pub struct HardwareQueryGate(Arc<AtomicBool>);

struct Release(Arc<AtomicBool>);
impl Drop for Release {
    fn drop(&mut self) {
        self.0.store(false, Ordering::SeqCst);
    }
}

impl HardwareQueryGate {
    pub async fn run<T: Send + 'static>(
        &self,
        work: impl FnOnce() -> Result<T, String> + Send + 'static,
    ) -> Result<T, String> {
        self.run_with_timeout(Duration::from_millis(600), work)
            .await
    }

    async fn run_with_timeout<T: Send + 'static>(
        &self,
        timeout: Duration,
        work: impl FnOnce() -> Result<T, String> + Send + 'static,
    ) -> Result<T, String> {
        self.0
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .map_err(|_| {
                "Microphone volume is still responding; wait before retrying".to_string()
            })?;
        let flag = self.0.clone();
        let task = tauri::async_runtime::spawn_blocking(move || {
            let _release = Release(flag);
            work()
        });
        tokio::time::timeout(timeout, task)
            .await
            .map_err(|_| {
                "Microphone volume query timed out; the device may still be responding".to_string()
            })?
            .map_err(|error| format!("Microphone volume query failed: {error}"))?
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn slow_hardware_does_not_hold_the_caller_or_create_more_workers() {
        let gate = HardwareQueryGate::default();
        let (release, receiver) = std::sync::mpsc::channel();
        let first = gate
            .run_with_timeout(Duration::from_millis(20), move || {
                receiver.recv().unwrap();
                Ok(1)
            })
            .await;
        assert!(first.unwrap_err().contains("timed out"));
        let second = gate
            .run(|| -> Result<i32, String> { panic!("must not start a second hardware call") })
            .await;
        assert!(second.unwrap_err().contains("still responding"));
        release.send(()).unwrap();
        for _ in 0..100 {
            if !gate.0.load(Ordering::SeqCst) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(2)).await;
        }
        assert_eq!(gate.run(|| Ok(2)).await.unwrap(), 2);
    }
}
