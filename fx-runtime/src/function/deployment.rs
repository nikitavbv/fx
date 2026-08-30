use {
    std::{rc::Rc, pin::Pin, cell::RefCell},
    tracing::{debug, warn, error},
    thiserror::Error,
    serde::{Serialize, Deserialize},
    crate::{
        triggers::http::HttpBody,
        resources::future::{FunctionBackgroundTask, FunctionResponseFuture},
        function::instance::{RuntimeServices, InstanceBindings, function_response_poll::FunctionResponsePollError},
    },
    super::instance::{FunctionInstanceState, FunctionInstance, FunctionInstanceInitError},
};

/// deployment is a set of FunctionInstances deployed with same configuration
#[derive(Debug, Clone, Hash, Eq, PartialEq)]
pub(crate) struct FunctionDeploymentId {
    id: u64,
}

impl FunctionDeploymentId {
    pub(crate) fn new(id: u64) -> Self {
        Self { id }
    }
}

pub(crate) struct FunctionDeployment {
    pub(crate) function_id: FunctionId,
    template: FunctionTemplate,
    pub(crate) instance: RefCell<Rc<FunctionInstance>>,
}

impl FunctionDeployment {
    pub async fn new(
        wasmtime: Rc<wasmtime::Engine>,
        runtime_services: RuntimeServices,
        limit_memory_bytes: Option<usize>,
        function_id: FunctionId,
        module: wasmtime::Module,
        bindings: InstanceBindings,
    ) -> Result<Self, DeploymentInitError> {
        fn add_imported_function<Params, Args>(linker: &mut wasmtime::Linker<FunctionInstanceState>, name: &'static str, func: impl wasmtime::IntoFunc<FunctionInstanceState, Params, Args>) -> Result<(), DeploymentInitError> {
            linker.func_wrap("fx", name, func)
                .map(|_| ())
                .map_err(|err| {
                    // not expecting error because there is only one module (fx) with a statically defined list of functions,
                    // so shadowing is not possible
                    error!("didn't expect error when adding imported function to linker: {err:?}");
                    DeploymentInitError::AssertionError
                })
        }

        macro_rules! add_imported_functions {
            ($linker:expr, $($name:literal => $func:expr);* $(;)?) => {
                $(add_imported_function($linker, $name, $func)?);*
            };
        }

        let mut linker = wasmtime::Linker::<FunctionInstanceState>::new(&wasmtime);

        add_imported_functions!(&mut linker,
            "fx_log" => super::abi::fx_log_handler;
            "fx_sql_exec" => super::abi::fx_sql_exec_handler;
            "fx_sql_batch" => super::abi::fx_sql_batch_handler;
            "fx_sql_migrate" => super::abi::fx_sql_migrate_handler;
            "fx_sleep" => super::abi::fx_sleep_handler;
            "fx_random" => super::abi::fx_random_handler;
            "fx_time" => super::abi::fx_time_handler;
            "fx_blob_put" => super::abi::fx_blob_put_handler;
            "fx_fetch" => super::abi::fx_fetch_handler;
            "fx_metrics_counter_register" => super::abi::fx_metrics_counter_register_handler;
            "fx_metrics_counter_increment" => super::abi::fx_metrics_counter_increment_handler;
            "fx_metrics_gauge_update" => super::abi::fx_metrics_gauge_update;
            "fx_env_len" => super::abi::fx_env_len_handler;
            "fx_env_get" => super::abi::fx_env_get_handler;
            "fx_kv_get" => super::abi::fx_kv_get_handler;
            "fx_kv_delex_ifeq" => super::abi::fx_kv_delex_ifeq_handler;
            "fx_kv_delex_result_future_poll" => super::abi::fx_kv_delex_result_future_poll;
            "fx_kv_delex_result_serialize" => super::abi::fx_kv_delex_result_serialize;
            "fx_kv_subscribe" => super::abi::fx_kv_subscribe_handler;
            "fx_kv_publish" => super::abi::fx_kv_publish_handler;
            "fx_tasks_background_spawn" => super::abi::fx_tasks_background_spawn_handler;
            "fx_fetch_request_header_serialize" => super::abi::fx_fetch_request_header_serialize_handler;
            "fx_bytes_len" => super::abi::fx_bytes_len_handler;
            "fx_bytes_move" => super::abi::fx_bytes_move_handler;
            "fx_kv_subscription_stream_poll_next" => super::abi::fx_kv_subscription_stream_poll_next;
            "fx_kv_publish_result_future_poll" => super::abi::fx_kv_publish_result_future_poll;
            "fx_kv_publish_result_serialize" => super::abi::fx_kv_publish_result_serialize;
            "fx_unit_future_poll" => super::abi::fx_unit_future_poll;
            "fx_sql_query_result_future_poll" => super::abi::fx_sql_query_result_future_poll;
            "fx_sql_query_result_serialize" => super::abi::fx_sql_query_result_serialize;
            "fx_sql_batch_result_future_poll" => super::abi::fx_sql_batch_result_future_poll;
            "fx_sql_batch_result_serialize" => super::abi::fx_sql_batch_result_serialize;
            "fx_migration_result_future_poll" => super::abi::fx_migration_result_future_poll;
            "fx_migration_result_serialize" => super::abi::fx_migration_result_serialize;
            "fx_fetch_result_future_poll" => super::abi::fx_fetch_result_future_poll;
            "fx_fetch_result_serialize" => super::abi::fx_fetch_result_serialize;
            "fx_http_body_poll_frame" => super::abi::fx_http_body_poll_frame;
            "fx_http_frame_serialize" => super::abi::fx_http_frame_serialize;
            "fx_blob_put_result_poll" => super::abi::fx_blob_put_result_poll;
            "fx_blob_put_result_serialize" => super::abi::fx_blob_put_result_serialize;
        );

        for import in module.imports() {
            if import.module() == "fx" {
                continue;
            }

            if let Some(f) = import.ty().func() {
                let result = linker.func_new(
                    import.module(),
                    import.name(),
                    f.clone(),
                    move |_, _, _| {
                        Err(wasmtime::Error::msg("requested function is not implemented by fx runtime"))
                    }
                );

                if let Err(err) = result {
                    error!("unknown error when definining a placeholder import: {err:?}");
                    return Err(DeploymentInitError::UnknownInstantiationError);
                }
            }
        }

        let instance_template = linker.instantiate_pre(&module)
            .map_err(|err| {
                if err.downcast_ref::<wasmtime::UnknownImportError>().is_some() {
                    return DeploymentInitError::MissingImport;
                }
                let err_str = err.to_string();
                if err_str.contains("incompatible import type") {
                    return DeploymentInitError::IncompatibleImport { details: err_str };
                }
                error!("unexpected error during module instantiation: {err:?}");
                DeploymentInitError::UnknownInstantiationError
            })?;

        let template = FunctionTemplate::new(
            wasmtime,
            runtime_services,
            limit_memory_bytes,
            function_id.clone(),
            instance_template,
            bindings,
        );

        let instance = template.instantiate().await.map_err(|err| match err {
            FunctionInstanceInitError::MissingExport => DeploymentInitError::MissingExport,
            FunctionInstanceInitError::MissingMemory => DeploymentInitError::MissingMemory,
            FunctionInstanceInitError::UnknownError => DeploymentInitError::UnknownInstantiationError,
            FunctionInstanceInitError::InternalRuntimeAssertionError => DeploymentInitError::AssertionError,
        })?;

        Ok(Self {
            function_id,
            template,
            instance: RefCell::new(instance),
        })
    }
}

pub(crate) mod handle_request {
    use super::*;

    pub(crate) struct RequestHandleFuture {
        inner: Pin<Box<dyn Future<Output = Result<http::Response<HttpBody>, FunctionDeploymentHandleRequestError>>>>,
        pub(crate) progress: Rc<RefCell<RequestHandleProgress>>,
    }

    impl RequestHandleFuture {
        fn new(deployment: Rc<FunctionDeployment>, header: http::Request<()>, body: Option<HttpBody>) -> Self {
            let instance = deployment.instance.clone();
            let progress = Rc::new(RefCell::new(RequestHandleProgress::Init));

            let inner = {
                let progress = progress.clone();

                Box::pin(async move {
                    let instance = if *instance.borrow().has_panicked.borrow() {
                        let instance = match deployment.template.instantiate().await.map_err(FunctionDeploymentHandleRequestError::from) {
                            Ok(v) => v,
                            Err(err) => return Err(err),
                        };
                        *deployment.instance.borrow_mut() = instance.clone();
                        instance
                    } else {
                        instance.borrow().clone()
                    };
                    *progress.borrow_mut() = RequestHandleProgress::InstanceReady;

                    debug!("inside request handling");
                    let resource = {
                        let mut data = match instance.store_lock().await {
                            Ok(v) => v,
                            Err(_) => {
                                warn!("timeout when acquiring lock to insert http body resource");
                                return Err(FunctionDeploymentHandleRequestError::RuntimeTimeout);
                            }
                        };
                        let data = data.data_mut();
                        data.resource_set.fetch_request_headers.insert(http::Request::from_parts(header.into_parts().0, body.map(|v| data.resource_set.http_bodies.insert(v))))
                    };
                    *progress.borrow_mut() = RequestHandleProgress::HttpBodyReady;

                    debug!("resource obtained");
                    let result = FunctionResponseFuture::new(
                        instance.clone(),
                        match instance.invoke_http_trigger(&resource).await.map_err(FunctionDeploymentHandleRequestError::from) {
                            Ok(v) => v,
                            Err(FunctionDeploymentHandleRequestError::FunctionPanicked) => {
                                *instance.has_panicked.borrow_mut() = true;
                                return Err(FunctionDeploymentHandleRequestError::FunctionPanicked);
                            },
                            Err(err) => return Err(err),
                        },
                    ).await;
                    *progress.borrow_mut() = RequestHandleProgress::HttpTriggerInvoked;

                    debug!("invoke_http_trigger called");
                    {
                        let mut function_state = instance.store.lock().await;
                        let function_state = function_state.data_mut();
                        debug!("draining background tasks");
                        for background_task in function_state.tasks_background.drain(..) {
                            tokio::task::spawn_local(FunctionBackgroundTask::new(instance.clone(), background_task));
                        }
                        debug!("drained background tasks");
                    }
                    *progress.borrow_mut() = RequestHandleProgress::BackgroundTasksSpawned;

                    debug!("function future created");
                    let result = result
                        .map_err(|err| match err {
                            FunctionResponsePollError::AssertionError => FunctionDeploymentHandleRequestError::AssertionError,
                            FunctionResponsePollError::FunctionPanicked => FunctionDeploymentHandleRequestError::FunctionPanicked,
                            FunctionResponsePollError::FunctionCrashed => FunctionDeploymentHandleRequestError::FunctionCrashed,
                            FunctionResponsePollError::AbiError
                            | FunctionResponsePollError::FailedToDeserialize
                            | FunctionResponsePollError::HostResourceNotFound
                            | FunctionResponsePollError::InvalidStatusCode
                            | FunctionResponsePollError::InvalidHeaders => FunctionDeploymentHandleRequestError::FunctionIncorrectResponse,
                        });
                    *progress.borrow_mut() = RequestHandleProgress::ResultFutureCreated;

                    result
                })
            };

            Self {
                inner,
                progress,
            }
        }
    }

    impl Future for RequestHandleFuture {
        type Output = Result<http::Response<HttpBody>, FunctionDeploymentHandleRequestError>;

        fn poll(mut self: Pin<&mut Self>, cx: &mut std::task::Context<'_>) -> std::task::Poll<Self::Output> {
            self.inner.as_mut().poll(cx)
        }
    }

    #[derive(Debug, Clone)]
    pub(crate) enum RequestHandleProgress {
        Init,
        InstanceReady,
        HttpBodyReady,
        HttpTriggerInvoked,
        BackgroundTasksSpawned,
        ResultFutureCreated,
    }

    impl FunctionDeployment {
        pub(crate) fn handle_request(self: &Rc<Self>, header: http::Request<()>, body: Option<HttpBody>) -> RequestHandleFuture {
            RequestHandleFuture::new(self.clone(), header, body)
        }
    }
}

#[derive(Debug, Error)]
pub(crate) enum DeploymentInitError {
    #[error("function requested import that fx runtime does not provide")]
    MissingImport,
    #[error("incompatible import type - was the function compiled with a different fx sdk version? {details:?}")]
    IncompatibleImport { details: String },
    #[error("function does not provide export that fx runtime expects")]
    MissingExport,
    #[error("function does not provide memory export that fx runtime expects")]
    MissingMemory,
    #[error("failed to create function instance because of unknown instantiation error")]
    UnknownInstantiationError,
    #[error("internal runtime assertion error")]
    AssertionError,
}

#[derive(Debug, Error)]
pub enum FunctionDeploymentHandleRequestError {
    #[error("internal runtime assertion error")]
    AssertionError,
    #[error("internal timeout in runtime implementation")]
    RuntimeTimeout,
    /// Function panicked while handling request
    #[error("function panicked")]
    FunctionPanicked,
    #[error("function stopped execution with unknown wasm trap")]
    FunctionCrashed,
    /// Function is busy handling other requests and cannot accept a new one
    #[error("function busy handling other requests and cannot accept a new one")]
    FunctionBusy,
    #[error("function returned incorrect response")]
    FunctionIncorrectResponse,
    #[error("failed to create function instance")]
    FunctionInstantiationError,
}

impl From<crate::function::instance::invoke_http_trigger::InvokeError> for FunctionDeploymentHandleRequestError {
    fn from(err: crate::function::instance::invoke_http_trigger::InvokeError) -> Self {
        use crate::function::instance::invoke_http_trigger::InvokeError;
        match err {
            InvokeError::Busy => Self::FunctionBusy,
            InvokeError::Panicked => Self::FunctionPanicked,
            InvokeError::Crashed => Self::FunctionCrashed,
        }
    }
}

impl From<FunctionInstanceInitError> for FunctionDeploymentHandleRequestError {
    fn from(_err: FunctionInstanceInitError) -> Self {
        Self::FunctionInstantiationError
    }
}

#[derive(Hash, Eq, PartialEq, Clone, Serialize, Deserialize, Debug)]
pub struct FunctionId {
    id: String,
}

impl FunctionId {
    pub fn new(id: impl Into<String>) -> Self {
        Self {
            id: id.into(),
        }
    }

    pub fn as_string(&self) -> String {
        self.id.clone()
    }

    pub fn as_str(&self) -> &str {
        self.id.as_str()
    }
}

impl From<FunctionId> for String {
    fn from(value: FunctionId) -> Self {
        value.id
    }
}

impl From<&FunctionId> for String {
    fn from(value: &FunctionId) -> Self {
        value.id.clone()
    }
}

struct FunctionTemplate {
    wasmtime: Rc<wasmtime::Engine>,
    runtime_services: RuntimeServices,
    limit_memory_bytes: Option<usize>,
    function_id: FunctionId,
    instance_template: wasmtime::InstancePre<FunctionInstanceState>,
    bindings: InstanceBindings,
}

impl FunctionTemplate {
    pub fn new(
        wasmtime: Rc<wasmtime::Engine>,
        runtime_services: RuntimeServices,
        limit_memory_bytes: Option<usize>,
        function_id: FunctionId,
        instance_template: wasmtime::InstancePre<FunctionInstanceState>,
        bindings: InstanceBindings,
    ) -> Self {
        Self {
            wasmtime,
            runtime_services,
            limit_memory_bytes,
            function_id,
            instance_template,
            bindings,
        }
    }

    pub async fn instantiate(&self) -> Result<Rc<FunctionInstance>, FunctionInstanceInitError> {
        FunctionInstance::new(
            &self.wasmtime,
            self.runtime_services.clone(),
            self.limit_memory_bytes,
            self.function_id.clone(),
            &self.instance_template,
            self.bindings.clone(),
        ).await
    }
}
