use {
    std::task::Poll,
    fx_api::{capnp, fx_capnp},
    futures::TryFutureExt,
    crate::{
        fx_futures::{FUTURE_POOL, PoolIndex, FxFuture, FunctionFutureError},
        fx_streams::STREAM_POOL,
        handler::HANDLERS,
        error::FxError,
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
        Poll::Ready(Ok(v)) => {
            response.set_ready(&v);
        },
        Poll::Ready(Err(err)) => {
            let mut error = response.init_error().init_error();
            match err {
                FunctionFutureError::UserApplicationError { description } => error.set_user_application_error(description),
                _other => error.set_internal_runtime_error(()),
            }
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

pub(crate) fn handle_invoke(
    invoke_request: fx_capnp::function_invoke_request::Reader,
    invoke_response: fx_capnp::function_invoke_response::Builder
) {
    let mut result = invoke_response.init_result();

    let handler_name = match invoke_request.get_method() {
        Ok(v) => v,
        Err(_err) => {
            let mut error = result.init_error().init_error();
            error.set_internal_runtime_error(());
            return;
        }
    };
    let handler_name = match handler_name.to_str() {
        Ok(v) => v,
        Err(_err) => {
            let mut error = result.init_error().init_error();
            error.set_bad_request(());
            return;
        }
    };

    let handler = match HANDLERS.get(&handler_name) {
        Some(v) => v,
        None => {
            let mut error = result.init_error().init_error();
            error.set_handler_not_found(());
            return;
        }
    };

    let handler_future = handler(invoke_request.get_payload().unwrap().to_vec());
    let handler_future = FxFuture::wrap(
        handler_future
            .map_err(|err| FunctionFutureError::from(err))
    );
    result.set_future_id(handler_future.future_index());
}
