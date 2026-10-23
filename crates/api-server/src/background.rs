//! Background loops that outlive a panic in one of their passes.

use std::any::Any;
use std::future::Future;
use std::panic::AssertUnwindSafe;

use futures_util::FutureExt;

/// Run one pass of the background loop `task`. A panic in it ends only that
/// pass and is logged, so the loop does not stop for the rest of the
/// process's life with nothing said. Each pass starts from the database.
/// Returns whether the pass ran to its end.
pub(crate) async fn pass<F: Future<Output = ()>>(task: &'static str, work: F) -> bool {
    match AssertUnwindSafe(work).catch_unwind().await {
        Ok(()) => true,
        Err(panic) => {
            tracing::error!(
                task,
                panic = %panic_message(panic.as_ref()),
                "A background pass panicked; the loop goes on"
            );
            false
        }
    }
}

fn panic_message(panic: &(dyn Any + Send)) -> String {
    panic
        .downcast_ref::<&str>()
        .map(|message| message.to_string())
        .or_else(|| panic.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "unknown panic".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[tokio::test]
    async fn a_panicking_pass_does_not_end_the_loop() {
        let passes = AtomicUsize::new(0);
        for round in 0..3 {
            let finished = pass("test", async {
                passes.fetch_add(1, Ordering::SeqCst);
                if round == 1 {
                    panic!("odd input");
                }
            })
            .await;
            assert_eq!(finished, round != 1);
        }
        assert_eq!(passes.load(Ordering::SeqCst), 3);
        assert_eq!(panic_message(&"text"), "text");
        assert_eq!(panic_message(&String::from("owned")), "owned");
        assert_eq!(panic_message(&7_u8), "unknown panic");
    }
}
