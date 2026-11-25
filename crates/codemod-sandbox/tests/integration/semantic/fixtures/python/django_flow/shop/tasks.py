from celery import shared_task


@shared_task
def notify(customer, count):
    return customer
