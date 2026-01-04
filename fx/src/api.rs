use {
    std::task::Poll,
    fx_api::{capnp, fx_capnp},
    crate::fx_futures::{FUTURE_POOL, PoolIndex},
};

pub(crate) fn handle_future_poll(
    future_poll_request: fx_capnp::function_future_poll_request::Reader,
    future_poll_response: fx_capnp::function_future_poll_response::Builder
) {
    let mut response = future_poll_response.init_response();

    match FUTURE_POOL.poll(PoolIndex(future_poll_request.get_future_id())) {
        Poll::Pending => {
            response.set_pending(());
        },
        Poll::Ready(v) => {
            response.set_ready(&v);
        }
    }
}
