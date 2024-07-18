use crate::utils::{test_address, test_rpc_builder};
use futures::future::BoxFuture;
use jsonrpsee::{
    rpc_params,
    server::{middleware::rpc::RpcServiceT, RpcServiceBuilder},
    types::Request,
    MethodResponse,
};
use reth_rpc::EthApi;
use reth_rpc_builder::{RpcServerConfig, TransportRpcModuleConfig};
use reth_rpc_eth_api::EthApiClient;
use reth_rpc_server_types::RpcModuleSelection;
use std::{
    future::Future,
    pin::Pin,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
};
use tower::Layer;



impl<S> Layer<S> for MyMiddlewareLayer {
    type Service = MyMiddlewareService<S>;

    fn layer(&self, inner: S) -> Self::Service {
        MyMiddlewareService { service: inner, count: self.count.clone() }
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

#[derive(Clone, Default)]
struct MyMiddlewareLayer {
    count: Arc<AtomicUsize>,
}







#[derive(Clone)]
#[allow(missing_debug_implementations, missing_docs)]
pub struct MyMiddleware<S> {
    pub service: S,
    pub count: Arc<AtomicUsize>,
}

impl<'a, S> RpcServiceT<'a> for MyMiddleware<S>
where
    S: RpcServiceT<'a> + Send + Sync + Clone + 'static,
{
    type Future = BoxFuture<'a, MethodResponse>;

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

impl<S> Layer<S> for MyMiddleware<S> {
    type Service = MyMiddleware<S>;

    fn layer(&self, inner: S) -> Self::Service {
        MyMiddleware { service: inner, count: self.count.clone() }
    }
}

impl<S> Default for MyMiddleware<S>
where
    S: Default,
{
    fn default() -> Self {
        MyMiddleware {
            service: S::default(),
            count: Arc::new(AtomicUsize::new(0)),
        }
    }
}


#[tokio::test(flavor = "multi_thread")]
async fn test_rpc_middleware() {
    let builder = test_rpc_builder();
    let modules = builder.build(
        TransportRpcModuleConfig::set_http(RpcModuleSelection::All),
        Box::new(EthApi::with_spawner),
    );

    use reth_rpc_builder::metrics::RpcRequestMetrics;
    use reth_rpc_builder::metrics::RpcRequestMetricsService;

    let metrics = RpcRequestMetrics::default();
    let rpc_service_instance = RpcRequestMetricsService::new(metrics, Default::default()); 

    //let mylayer = MyMiddlewareLayer::default();
    let mylayer = MyMiddleware {
        service: rpc_service_instance,
        count: Arc::new(AtomicUsize::new(0)),
    };

    let rpc_middleware = RpcServiceBuilder::new().layer(mylayer.clone());
    //.layer_fn(move |service: ()| MyMiddleware { service, count: Arc::new(AtomicUsize::new(0)) });

    let handle = RpcServerConfig::http(Default::default())
        .with_http_address(test_address())
        .set_rpc_middleware(rpc_middleware)
        .start(&modules)
        .await
        .unwrap();

    let client = handle.http_client().unwrap();
    EthApiClient::protocol_version(&client).await.unwrap();
    let count = mylayer.count.load(Ordering::Relaxed);
    assert_eq!(count, 1);
}
