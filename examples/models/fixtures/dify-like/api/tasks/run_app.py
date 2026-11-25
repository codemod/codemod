from celery import shared_task


@shared_task(queue="app_run")
def run_app_task(app_id, inputs):
    return {"app": app_id, "inputs": inputs}
