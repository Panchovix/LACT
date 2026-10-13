# Synthetic protocol peer. Never imports or opens a GPU driver.
import hashlib
from pathlib import Path
path = Path(state_dir) / "profile.json"
state = json.loads(path.read_text()) if path.exists() else {"checkpoint": None, "receipts": {}, "native_pending": False}
mode_path = Path(state_dir) / "mode"
mode = mode_path.read_text().strip() if mode_path.exists() else ""
with (Path(state_dir) / "calls").open("a") as out:
    out.write(r["action"] + "\n")
base = {"protocol_version": 1, "available": True, "managed": False, "device": r["device"], "rails": [{"rail": "NVVDD", "max_voltage": {"evaluated_uv": 950000, "offset_uv": 0, "writable": True}}], "capabilities": {"profile_transactions": 1}, "recovery_required": False}
def save():
    path.write_text(json.dumps(state))
def emit(receipt=None, pending=False):
    reply = dict(base)
    if receipt:
        reply.update(receipt)
    if pending:
        reply.update(available=False, managed=True, recovery_required=True)
    if receipt and mode == "wrong_receipt_request":
        reply["request_id"] = "f" * 64
    print(json.dumps(reply))
    sys.exit(0)
def fail(message):
    sys.stderr.write(message)
    sys.exit(2)
if r["action"] == "status":
    if mode == "status_failure":
        fail("status unavailable")
    if mode == "status_once":
        counter = Path(state_dir) / "status_count"
        count = int(counter.read_text()) if counter.exists() else 0
        counter.write_text(str(count + 1))
        if count:
            fail("status unavailable after the first reply")
    token = r.get("transaction_id")
    if token:
        receipt = state["checkpoint"] or state["receipts"].get(token)
        if not receipt or receipt["transaction_id"] != token:
            fail("unknown receipt")
        emit(receipt, receipt["transaction_status"] not in ("committed", "rolled_back"))
    if state["checkpoint"]:
        emit(state["checkpoint"], True)
    if state["native_pending"]:
        base.update(available=False, managed=True, recovery_required=True)
    emit()
if r["action"] == "stage":
    if state["checkpoint"]:
        fail("profile recovery required")
    if mode == "native_pending_without_receipt":
        state["native_pending"] = True
        save()
        fail("native recovery required")
    empty = not any(value is not None for rail in (r.get("settings") or {}).values() for value in rail.values())
    if empty or mode == "bad_noop":
        base.update(available=False, transaction_status="noop", request_id=r["request_id"])
        emit()
    receipt = {"transaction_id": hashlib.sha256(r["request_id"].encode()).hexdigest(), "request_id": r["request_id"], "transaction_status": "staged"}
    state["checkpoint"] = receipt
    save()
    if mode == "lost_stage_reply":
        fail("stage reply lost after completion")
    emit(receipt)
if r["action"] in ("commit", "rollback"):
    token = r["transaction_id"]
    receipt = state["checkpoint"] or state["receipts"].get(token)
    if not receipt or receipt["transaction_id"] != token:
        fail("unknown receipt")
    if mode == "rollback_failure" and r["action"] == "rollback":
        fail("rollback unconfirmed")
    terminal = "committed" if r["action"] == "commit" else "rolled_back"
    receipt = dict(receipt, transaction_status=terminal)
    state["receipts"][token] = receipt
    state["checkpoint"] = None
    save()
    if mode == "lost_commit_reply" and r["action"] == "commit":
        fail("commit reply lost after completion")
    emit(receipt)
if r["action"] == "restore":
    if state["checkpoint"]:
        fail("profile recovery required")
    if mode == "restore_failure":
        fail("restore unconfirmed")
    state["native_pending"] = False
    save()
    emit()
fail("unsupported synthetic action")
