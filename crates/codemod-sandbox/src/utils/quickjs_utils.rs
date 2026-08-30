pub async fn maybe_promise<'js>(
    result_obj: rquickjs::Value<'js>,
) -> rquickjs::Result<rquickjs::Value<'js>> {
    let resolved_obj: rquickjs::Value = if result_obj.is_promise() {
        let promise = result_obj.as_promise().unwrap().clone();
        let ctx = result_obj.ctx();
        while ctx.execute_pending_job() {}

        promise.into_future::<rquickjs::Value<'js>>().await?
    } else {
        result_obj.clone()
    };
    Ok(resolved_obj)
}
