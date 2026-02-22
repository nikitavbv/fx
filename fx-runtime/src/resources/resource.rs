/// Function resource handle that is owned by host.
/// Cleans up function memory if dropped before being consumed
pub struct OwnedFunctionResourceId(Cell<Option<(Rc<FunctionInstance>, FunctionResourceId)>>);

impl OwnedFunctionResourceId {
    pub fn new(function_instance: Rc<FunctionInstance>, resource_id: FunctionResourceId) -> Self {
        Self(Cell::new(Some((function_instance, resource_id))))
    }

    pub fn consume(self) -> (Rc<FunctionInstance>, FunctionResourceId) {
        self.0.replace(None).unwrap()
    }
}

impl Drop for OwnedFunctionResourceId {
    fn drop(&mut self) {
        if let Some((function_instance, resource_id)) = self.0.replace(None) {
            tokio::task::spawn_local(async move {
                function_instance.resource_drop(&resource_id).await;
            });
        }
    }
}

trait DeserializeFunctionResource {
    fn deserialize(resource: &mut &[u8], instance: Rc<FunctionInstance>) -> Self;
}

impl DeserializeFunctionResource for FunctionResponse {
    fn deserialize(resource: &mut &[u8], instance: Rc<FunctionInstance>) -> Self {
        let message_reader = capnp::serialize::read_message_from_flat_slice(resource, capnp::message::ReaderOptions::default()).unwrap();
        let response = message_reader.get_root::<abi_function_resources_capnp::function_response::Reader>().unwrap();
        Self(FunctionResponseInner::HttpResponse(FunctionHttpResponse {
            status: ::http::StatusCode::from_u16(response.get_status()).unwrap(),
            body: Cell::new(Some(SerializedFunctionResource::new(instance, FunctionResourceId::from(response.get_body_resource())))),
        }))
    }
}

impl DeserializeFunctionResource for Vec<u8> {
    fn deserialize(resource: &mut &[u8], _instance: Rc<FunctionInstance>) -> Self {
        resource.to_vec()
    }
}

pub(crate) struct ResourceId {
    id: u64,
}

impl ResourceId {
    pub fn new(id: u64) -> Self {
        Self { id }
    }

    pub fn as_u64(&self) -> u64 {
        self.id
    }
}

impl From<slotmap::DefaultKey> for ResourceId {
    fn from(value: slotmap::DefaultKey) -> Self {
        Self::new(value.data().as_ffi())
    }
}

impl Into<slotmap::DefaultKey> for &ResourceId {
    fn into(self) -> slotmap::DefaultKey {
        slotmap::DefaultKey::from(slotmap::KeyData::from_ffi(self.id))
    }
}

impl From<u64> for ResourceId {
    fn from(id: u64) -> Self {
        Self { id }
    }
}

#[derive(Clone, Debug)]
pub(crate) struct FunctionResourceId {
    id: u64,
}

impl FunctionResourceId {
    pub fn new(id: u64) -> Self {
        Self { id }
    }

    pub fn as_u64(&self) -> u64 {
        self.id
    }
}

impl From<u64> for FunctionResourceId {
    fn from(id: u64) -> Self {
        Self { id }
    }
}

pub(crate) enum Resource {
    FetchRequest(SerializableResource<FetchRequestHeader>),
    RequestBody(FetchRequestBody),
    SqlQueryResult(FutureResource<SerializableResource<SqlQueryResult>>),
    SqlMigrationResult(FutureResource<SerializableResource<SqlMigrationResult>>),
    UnitFuture(BoxFuture<'static, ()>),
    BlobGetResult(FutureResource<SerializableResource<BlobGetResponse>>),
    FetchResult(FutureResource<SerializableResource<FetchResult>>),
}
