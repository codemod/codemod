from celery import shared_task


@shared_task(name="mail.send_invite")
def send_invite(email, app_id):
    print("invite", email, app_id)


@shared_task
def send_digest(email):
    print("digest", email)
