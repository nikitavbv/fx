use {
    std::task::Poll,
    fx_api::{capnp, fx_capnp},
    crate::{
        fx_futures::{FUTURE_POOL, PoolIndex},
        fx_streams::STREAM_POOL,
    },
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

pub(crate) fn handle_future_drop(
    future_drop_request: fx_capnp::function_future_drop_request::Reader,
    future_drop_response: fx_capnp::function_future_drop_response::Builder
) {
    let mut response = future_drop_response.init_response();

    match FUTURE_POOL.remove(PoolIndex(future_drop_request.get_future_id())) {
        Ok(_) => response.set_ok(()),
        Err(_err) => response.set_error(()),
    }
}

pub(crate) fn handle_stream_poll_next(
    stream_next_request: fx_capnp::function_stream_poll_next_request::Reader,
    stream_next_response: fx_capnp::function_stream_poll_next_response::Builder
) {
    let mut response = stream_next_response.init_response();

    match STREAM_POOL.next(stream_next_request.get_stream_id() as i64) {
        Poll::Pending => response.set_pending(()),
        Poll::Ready(Some(v)) => response.set_ready(&v),
        Poll::Ready(None) => response.set_finished(()),
    }
}

pub(crate) fn handle_stream_drop(
    stream_drop_request: fx_capnp::function_stream_drop_request::Reader,
    _stream_drop_response: fx_capnp::function_stream_drop_response::Builder
) {
    STREAM_POOL.remove(stream_drop_request.get_stream_id());
}
