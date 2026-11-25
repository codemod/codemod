from flask import Blueprint, jsonify, request

from services.invite import invite_member
from tasks.run_app import run_app_task

bp = Blueprint("apps", __name__, url_prefix="/console/api")


@bp.route("/apps/<uuid:app_id>/run", methods=["POST"])
def run_app(app_id):
    payload = request.get_json()
    run_app_task.delay(app_id, payload["inputs"])
    return jsonify({"status": "queued"})


@bp.post("/apps/<uuid:app_id>/invite")
def invite(app_id):
    payload = request.get_json()
    invite_member(app_id, payload["email"])
    return jsonify({"status": "sent"})


@bp.get("/apps/<uuid:app_id>")
def get_app(app_id):
    return jsonify({"id": app_id})
