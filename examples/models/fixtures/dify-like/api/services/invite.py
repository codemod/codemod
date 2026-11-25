from extensions import celery_app


def invite_member(app_id, email):
    celery_app.send_task("mail.send_invite", args=[email, app_id])
