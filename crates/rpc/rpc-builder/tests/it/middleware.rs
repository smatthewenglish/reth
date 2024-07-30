use crate::utils::{test_address, test_rpc_builder};
use jsonrpsee::{
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

#[derive(Clone, Default)]
struct MyMiddlewareLayer {
    count: Arc<AtomicUsize>,
}

impl<S> Layer<S> for MyMiddlewareLayer {
    //type Service = MyMiddlewareService<S>;
    type Service = BasicService<S>;

    fn layer(&self, inner: S) -> Self::Service {
        //MyMiddlewareService { service: inner, count: self.count.clone() }
        BasicService { service: inner, count: self.count.clone() }
    }
}


struct BasicService<S> {
    service: S,
    count: Arc<AtomicUsize>,
}

impl<'a, S> RpcServiceT<'a> for BasicService<S> {
    type Future = Pin<Box<dyn Future<Output = MethodResponse> + Send + 'a>>;

    fn call(&self, req: Request<'a>) -> Self::Future {

        use jsonrpsee::types::params::Id;
        use jsonrpsee::ResponsePayload;

        Box::pin(async move {
            // Simulate some processing
            let id = Id::Number(1); // Example ID
            let result = ResponsePayload::success("Some result");
            let max_response_size = 1024; // Example size limit
            MethodResponse::response(id, result, max_response_size)
        })
    }
}




#[derive(Clone, Default)]
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



#[tokio::test(flavor = "multi_thread")]
async fn test_my_middleware() {

    use jsonrpsee::types::params::{Id, TwoPointZero};
    let two_point_zero: TwoPointZero = TwoPointZero::default();
    let id: Id<'_> = Id::Number(13);
    
    use beef::Cow;
    let method: Cow<'_, str> = Cow::owned("owned_string".to_string());

    use http::Extensions;
    let extensions: Extensions = Extensions::new();
    
    let request: Request<'_> = Request::new(
        method,
        None,
        id,
    );

    let my_layer = MyMiddlewareLayer::default();
    my_layer.call(request).await;


}



#[tokio::test(flavor = "multi_thread")]
async fn test_rpc_middleware() {
    let builder = test_rpc_builder();
    let modules = builder.build(
        TransportRpcModuleConfig::set_http(RpcModuleSelection::All),
        Box::new(EthApi::with_spawner),
    );

    let mylayer = MyMiddlewareLayer::default();

    let handle = RpcServerConfig::http(Default::default())
        .with_http_address(test_address())
        .set_rpc_middleware(RpcServiceBuilder::new().layer(mylayer.clone()))
        .start(&modules)
        .await
        .unwrap();

    let client = handle.http_client().unwrap();
    EthApiClient::protocol_version(&client).await.unwrap();
    let count = mylayer.count.load(Ordering::Relaxed);
    assert_eq!(count, 1);
}



