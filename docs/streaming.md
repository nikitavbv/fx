# streaming

TODO: add notes about how streams work

## current state of ABI

an open question is if all of the following are needed:
- `HttpRequestBody`
- `FunctionHttpBodyFrame`
  - used to serialize `FunctionResource::HttpBody` on sdk side
  - a weird mix: should not contain `hostResourceId` - that should be HttpBody, not frame.
- `HttpBodyFrame`
  - used for `fetch` requests
