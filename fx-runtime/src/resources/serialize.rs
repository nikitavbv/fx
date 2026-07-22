use {
    std::{marker::PhantomData, rc::Rc},
    thiserror::Error,
    crate::{
        function::{
            abi::{capnp, abi_http_capnp},
            instance::FunctionInstance,
        },
        triggers::http::HttpBody,
        resources::{resource::OwnedFunctionResourceId, FunctionResourceId, FunctionResources},
    },
};

/// Resource that origins from function side and is not owned by host.
/// moved lazily from function to host memory.
/// if dropped before being moved, cleans up resource on function side.
pub(crate) struct SerializedFunctionResource<T: DeserializeFunctionResource> {
    _t: PhantomData<T>,
    resource: OwnedFunctionResourceId,
}

impl<T: DeserializeFunctionResource> SerializedFunctionResource<T> {
    pub fn new(instance: Rc<FunctionInstance>, resource: FunctionResourceId) -> Self {
        Self {
            _t: PhantomData,
            resource: OwnedFunctionResourceId::new(instance, resource),
        }
    }

    pub(crate) async fn move_to_host(self) -> Result<T, <T as DeserializeFunctionResource>::Error> {
        let (instance, resource) = self.resource.consume();
        let serialized_resource = instance.move_serializable_resource_to_host(&resource).await;
        T::deserialize(&mut instance.clone().store.lock().await.data_mut().resource_set, &mut serialized_resource.as_slice(), instance)
    }
}

pub(crate) trait DeserializeFunctionResource {
    // having associated type here allows to have custom errors for different types (for example, additional validation while
    // deserializing or errors while resolving dependencies)
    type Error;

    fn deserialize(resource_set: &mut FunctionResources, resource: &mut &[u8], instance: Rc<FunctionInstance>) -> Result<Self, Self::Error> where Self: Sized;
}
