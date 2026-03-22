# streaming

TODO: add notes about how streams work

## current state of ABI

an open question is if all of the following are needed:
- `HttpRequestBody`
- `HttpRequestBodyFrame`
  - used for `HttpRequestBody` in sdk, which is body of `HttpRequest` (function input)
  - can probably be replaced with `HttpBodyFrame` (and `HttpBody` in sdk)
- `FunctionHttpBodyFrame`
- `HttpBodyFrame`
  - used for `fetch` requests
