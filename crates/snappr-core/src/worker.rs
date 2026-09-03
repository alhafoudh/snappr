//! One photo at a time.
//!
//! **This replaces something that used to be implicit.** The Ruby child accepted
//! one request at a time, and that queue - not any code in the shell - was what
//! stopped four 12MP composites running at once. The shell spawned an unbounded
//! thread per photo and got away with it because they all blocked on the same
//! pipe. Delete the child without replacing the queue and a busy AirDrop session
//! composites everything in parallel, each holding a 10-25 MB output plus its
//! source: the memory ceiling is what bites first, on the machine least able to
//! afford it.
//!
//! So: one thread, one queue, and callers wait on a channel for their own result.
//! `VipsImage` is `Send + Sync`, so this could grow to N workers if a queue ever
//! becomes the bottleneck - but a kiosk prints one photo at a time by definition,
//! and the operator is watching a filmstrip, not a throughput graph.
//!
//! Note what is *not* here: any knowledge of what the work is. The queue takes
//! closures, so the app can put a composite, a preview render and an overlay card
//! through the same lane without this module learning about any of them.

use std::sync::mpsc::{channel, Receiver, Sender};

type Job = Box<dyn FnOnce() + Send + 'static>;

/// A handle to the single worker thread. Cloneable, and dropping every clone
/// stops the thread once the queue drains.
#[derive(Clone)]
pub struct Worker {
    jobs: Sender<Job>,
}

impl Worker {
    /// Starts the worker thread.
    ///
    /// libvips is initialised here rather than by the caller, so that no job can
    /// be the first thing to touch it - the one ordering rule the library has.
    pub fn start() -> Result<Self, String> {
        crate::vips::init()?;

        let (jobs, queue) = channel::<Job>();
        std::thread::Builder::new()
            .name("snappr-pipeline".into())
            .spawn(move || {
                // Ends when every `Worker` handle has been dropped, which is what
                // makes the thread exit on shutdown without anybody signalling it.
                for job in queue {
                    job();
                }
                log::debug!("pipeline worker stopped");
            })
            .map_err(|error| format!("could not start the pipeline worker: {error}"))?;

        Ok(Self { jobs })
    }

    /// Queues `work` and hands back the channel its result will arrive on.
    ///
    /// The caller decides whether to block: a Tauri command awaits it, the
    /// watcher's callback ignores it. A dropped receiver is not an error - it
    /// means nobody is waiting any more, and the work still ran.
    pub fn submit<T, F>(&self, work: F) -> Receiver<T>
    where
        F: FnOnce() -> T + Send + 'static,
        T: Send + 'static,
    {
        let (reply, result) = channel::<T>();
        let job = Box::new(move || {
            let _ = reply.send(work());
        });

        if self.jobs.send(job).is_err() {
            // The worker thread is gone. The receiver will report the channel as
            // disconnected, which every caller already has to handle.
            log::error!("the pipeline worker is not running");
        }
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    #[test]
    fn work_runs_and_the_result_comes_back() {
        let worker = Worker::start().unwrap();
        let result = worker.submit(|| 2 + 2).recv().unwrap();
        assert_eq!(result, 4);
    }

    /// The property the sidecar used to provide by accident, now asserted: two
    /// jobs never overlap.
    #[test]
    fn jobs_never_run_concurrently() {
        let worker = Worker::start().unwrap();
        let inside = Arc::new(AtomicUsize::new(0));
        let worst = Arc::new(AtomicUsize::new(0));

        let waits: Vec<_> = (0..8)
            .map(|_| {
                let inside = Arc::clone(&inside);
                let worst = Arc::clone(&worst);
                worker.submit(move || {
                    let now = inside.fetch_add(1, Ordering::SeqCst) + 1;
                    worst.fetch_max(now, Ordering::SeqCst);
                    std::thread::sleep(std::time::Duration::from_millis(5));
                    inside.fetch_sub(1, Ordering::SeqCst);
                })
            })
            .collect();

        for wait in waits {
            wait.recv().unwrap();
        }
        assert_eq!(worst.load(Ordering::SeqCst), 1, "two jobs overlapped");
    }

    #[test]
    fn jobs_run_in_the_order_they_were_queued() {
        let worker = Worker::start().unwrap();
        let order = Arc::new(std::sync::Mutex::new(Vec::new()));

        let waits: Vec<_> = (0..5)
            .map(|index| {
                let order = Arc::clone(&order);
                worker.submit(move || order.lock().unwrap().push(index))
            })
            .collect();
        for wait in waits {
            wait.recv().unwrap();
        }

        assert_eq!(*order.lock().unwrap(), [0, 1, 2, 3, 4]);
    }

    /// A caller that stops waiting must not take the queue down with it.
    #[test]
    fn a_dropped_receiver_does_not_break_the_worker() {
        let worker = Worker::start().unwrap();
        drop(worker.submit(|| 1));
        assert_eq!(worker.submit(|| 2).recv().unwrap(), 2);
    }
}
