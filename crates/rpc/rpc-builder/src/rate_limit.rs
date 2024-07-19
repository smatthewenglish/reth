
use tower::limit::concurrency::ConcurrencyLimit;

use jsonrpsee::{
    server::middleware::rpc::RpcServiceT,
    types::Request,
    MethodResponse,
};
use std::{
    future::Future,
    pin::Pin,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
};
use tower::Layer;
use tokio::sync::{OwnedSemaphorePermit, Semaphore};
use tokio_util::sync::PollSemaphore;

// #[derive(Clone, Default)]
// struct MyMiddlewareLayer {
//     count: Arc<AtomicUsize>,
// }

/// Enforces a limit on the concurrent number of requests the underlying
/// service can handle.
#[derive(Debug)]
pub struct RateLimit<T> {
    inner: T,
    semaphore: PollSemaphore,
    /// The currently acquired semaphore permit, if there is sufficient
    /// concurrency to send a new request.
    ///
    /// The permit is acquired in `poll_ready`, and taken in `call` when sending
    /// a new request.
    permit: Option<OwnedSemaphorePermit>,
}

// impl<S> Layer<S> for MyMiddlewareLayer {
//     type Service = MyMiddlewareService<S>;

//     fn layer(&self, inner: S) -> Self::Service {
//         MyMiddlewareService { service: inner, count: self.count.clone() }
//     }
// }

impl<T> RateLimit<T> {
    /// Create a new concurrency limiter.
    pub fn new(inner: T, max: usize) -> Self {
        Self::with_semaphore(inner, Arc::new(Semaphore::new(max)))
    }

    /// Create a new concurrency limiter with a provided shared semaphore
    pub fn with_semaphore(inner: T, semaphore: Arc<Semaphore>) -> Self {
        ConcurrencyLimit {
            inner,
            semaphore: PollSemaphore::new(semaphore),
            permit: None,
        }
    }

    /// Get a reference to the inner service
    pub fn get_ref(&self) -> &T {
        &self.inner
    }

    /// Get a mutable reference to the inner service
    pub fn get_mut(&mut self) -> &mut T {
        &mut self.inner
    }

    /// Consume `self`, returning the inner service
    pub fn into_inner(self) -> T {
        self.inner
    }
}

#[derive(Clone)]
struct MyMiddlewareService<S> {
    service: S,
    count: Arc<AtomicUsize>,
}

impl<'a, S> RpcServiceT<'a> for MyMiddlewareService<S>
where
    S: RpcServiceT<'a> + Send + Sync + Clone + 'static,
{
    type Future = Pin<Box<dyn Future<Output = MethodResponse> + Send + 'a>>;

    fn call(&self, req: Request<'a>) -> Self::Future {
        tracing::info!("MyMiddleware processed call {}", req.method);
        let count = self.count.clone();
        let service = self.service.clone();
        Box::pin(async move {
            let rp = service.call(req).await;
            // Modify the state.
            count.fetch_add(1, Ordering::Relaxed);
            rp
        })
    }
}
