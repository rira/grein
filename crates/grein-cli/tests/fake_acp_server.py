#!/usr/bin/env python3
"""Deterministic fake ACP v1 NDJSON stdio server for integration testing."""

from __future__ import annotations

import argparse
import json
import os
import sys
import threading
import time
from typing import Any


class FakeAcpServer:
    def __init__(self, log_path: str | None) -> None:
        self._log_path = log_path
        self._stdout_lock = threading.Lock()
        self._log_lock = threading.Lock()
        self._state_lock = threading.Lock()
        self._session_counter = 0
        self._perm_counter = 0
        self._sessions: dict[str, dict[str, Any]] = {}
        self._perm_waiters: dict[str, tuple[threading.Event, dict[str, Any]]] = {}
        self._shutdown_event = threading.Event()
        self._slow_timeout_s = max(
            0.05, float(os.environ.get("FAKE_ACP_SLOW_MS", "1500")) / 1000.0
        )

    def _log_raw(self, line: str) -> None:
        if not self._log_path:
            return
        with self._log_lock:
            with open(self._log_path, "a", encoding="utf-8") as fh:
                fh.write(line.rstrip("\n") + "\n")
                fh.flush()

    def _send(self, payload: dict[str, Any]) -> None:
        encoded = json.dumps(payload, separators=(",", ":"))
        with self._stdout_lock:
            sys.stdout.write(encoded + "\n")
            sys.stdout.flush()

    def _send_response(self, req_id: Any, result: Any) -> None:
        self._send({"jsonrpc": "2.0", "id": req_id, "result": result})

    def _send_error(self, req_id: Any, code: int, message: str) -> None:
        self._send(
            {
                "jsonrpc": "2.0",
                "id": req_id,
                "error": {"code": code, "message": message},
            }
        )

    def _send_update(self, session_id: str, update: dict[str, Any]) -> None:
        self._send(
            {
                "jsonrpc": "2.0",
                "method": "session/update",
                "params": {"sessionId": session_id, "update": update},
            }
        )

    def _send_text_chunk(self, session_id: str, text: str) -> None:
        self._send_update(
            session_id,
            {
                "sessionUpdate": "agent_message_chunk",
                "content": {"type": "text", "text": text},
            },
        )

    def _ensure_session(self, session_id: str) -> dict[str, Any]:
        with self._state_lock:
            session = self._sessions.get(session_id)
            if session is None:
                session = {
                    "system_prompt": "",
                    "first_turn": True,
                    "cancel_event": threading.Event(),
                }
                self._sessions[session_id] = session
            return session

    def _handle_initialize(self, req_id: Any) -> None:
        self._send_response(
            req_id,
            {
                "protocolVersion": 1,
                "agentInfo": {"name": "Fake ACP Server", "version": "0.1.0"},
                "authMethods": [{"id": "oauth-personal", "name": "OAuth"}],
                "agentCapabilities": {
                    "loadSession": True,
                    "_meta": {"antigravity": {"systemPrompt": {"append": True}}},
                },
            },
        )

    def _handle_authenticate(self, req_id: Any) -> None:
        self._send_response(req_id, {})

    def _handle_session_new(self, req_id: Any, params: dict[str, Any]) -> None:
        meta = params.get("_meta") or {}
        ag = meta.get("antigravity") or {}
        sp = ag.get("systemPrompt") or {}
        append_text = sp.get("append")
        if not isinstance(append_text, str):
            append_text = ""

        with self._state_lock:
            self._session_counter += 1
            session_id = f"sess-{self._session_counter}"
            self._sessions[session_id] = {
                "system_prompt": append_text,
                "first_turn": True,
                "cancel_event": threading.Event(),
            }

        self._send_response(
            req_id,
            {
                "sessionId": session_id,
                "models": {"currentModelId": "fake-model", "availableModels": []},
                "modes": {"currentModeId": "default", "availableModes": []},
            },
        )

    def _extract_prompt_text(self, params: dict[str, Any]) -> str:
        prompt_items = params.get("prompt") or []
        if isinstance(prompt_items, str):
            return prompt_items
        parts: list[str] = []
        for item in prompt_items:
            if isinstance(item, dict):
                text = item.get("text")
                if isinstance(text, str):
                    parts.append(text)
        return "\n".join(parts)

    def _handle_permission_flow(
        self,
        session_id: str,
        cancel_event: threading.Event,
    ) -> str:
        with self._state_lock:
            self._perm_counter += 1
            perm_id = f"perm-{self._perm_counter}"
            wait_event = threading.Event()
            holder: dict[str, Any] = {}
            self._perm_waiters[perm_id] = (wait_event, holder)

        self._send(
            {
                "jsonrpc": "2.0",
                "id": perm_id,
                "method": "session/request_permission",
                "params": {
                    "sessionId": session_id,
                    "toolCall": {
                        "toolCallId": "call-1",
                        "name": "run_command",
                        "title": "echo ok",
                        "args": {},
                    },
                    "options": [
                        {
                            "optionId": "allow",
                            "name": "Allow",
                            "kind": "allow_once",
                        },
                        {
                            "optionId": "deny",
                            "name": "Deny",
                            "kind": "reject_once",
                        },
                    ],
                },
            }
        )

        while not wait_event.is_set():
            if cancel_event.is_set() or self._shutdown_event.is_set():
                with self._state_lock:
                    self._perm_waiters.pop(perm_id, None)
                return "cancelled"
            wait_event.wait(timeout=0.02)

        resp = holder.get("response") or {}
        result = resp.get("result") or {}
        outcome = result.get("outcome") or {}
        if outcome.get("outcome") == "selected":
            selected = outcome.get("optionId")
            if isinstance(selected, str) and selected:
                return selected
        return "cancelled"

    def _run_prompt_turn(self, req_id: Any, params: dict[str, Any]) -> None:
        session_id = params.get("sessionId")
        if not isinstance(session_id, str) or not session_id:
            self._send_error(req_id, -32602, "missing sessionId")
            return

        prompt_text = self._extract_prompt_text(params)
        session = self._ensure_session(session_id)

        cancel_event = threading.Event()
        with self._state_lock:
            session["cancel_event"] = cancel_event
            system_prompt = session["system_prompt"]

        if "DUMP_SYSTEM_PROMPT" in prompt_text:
            self._send_text_chunk(session_id, system_prompt)
            self._send_response(req_id, {"stopReason": "end_turn"})
            return

        if "ECHO_LAST_PROMPT" in prompt_text:
            self._send_text_chunk(session_id, prompt_text)
            self._send_response(req_id, {"stopReason": "end_turn"})
            return

        if "REQUIRE_PERMISSION" in prompt_text:
            option_id = self._handle_permission_flow(session_id, cancel_event)
            if cancel_event.is_set():
                self._send_response(req_id, {"stopReason": "cancelled"})
                return
            status = "completed" if option_id == "allow" else "failed"
            output = "ok" if option_id == "allow" else option_id
            self._send_update(
                session_id,
                {
                    "sessionUpdate": "tool_call",
                    "toolCallId": "call-1",
                    "name": "run_command",
                    "title": "echo ok",
                    "args": {},
                    "status": "pending",
                },
            )
            self._send_update(
                session_id,
                {
                    "sessionUpdate": "tool_call_update",
                    "toolCallId": "call-1",
                    "name": "run_command",
                    "status": status,
                    "rawOutput": {"output": output},
                },
            )
            self._send_text_chunk(session_id, f"PERMISSION:{option_id}")
            self._send_response(req_id, {"stopReason": "end_turn"})
            return

        if "STREAM_SLOW" in prompt_text:
            self._send_text_chunk(session_id, "PART1 ")
            deadline = time.monotonic() + self._slow_timeout_s
            while time.monotonic() < deadline:
                if cancel_event.is_set():
                    self._send_response(req_id, {"stopReason": "cancelled"})
                    return
                if self._shutdown_event.is_set():
                    return
                remaining = max(0.0, min(0.02, deadline - time.monotonic()))
                cancel_event.wait(timeout=remaining)
            if cancel_event.is_set():
                self._send_response(req_id, {"stopReason": "cancelled"})
                return
            self._send_text_chunk(session_id, "PART2")
            self._send_response(req_id, {"stopReason": "end_turn"})
            return

        if "SPAWN_SUBAGENT_WAKEUP" in prompt_text:
            self._send_update(
                session_id,
                {
                    "sessionUpdate": "tool_call",
                    "toolCallId": "tc-subagent-1",
                    "name": "invoke_subagent",
                    "title": "Launching background subagent",
                    "status": "in_progress",
                    "args": {
                        "Role": "Background Code Checker",
                        "TypeName": "research",
                        "Prompt": "check codebase",
                    },
                },
            )
            self._send_update(
                session_id,
                {
                    "sessionUpdate": "tool_call_update",
                    "toolCallId": "tc-subagent-1",
                    "status": "completed",
                },
            )
            self._send_text_chunk(session_id, "Spawned background subagent.")
            self._send_response(req_id, {"stopReason": "end_turn"})

            def _emit_wakeup() -> None:
                if self._shutdown_event.wait(timeout=0.15):
                    return
                with self._state_lock:
                    if session_id not in self._sessions:
                        return
                self._send_text_chunk(
                    session_id, "\n\nWAKEUP_OK: background subagent finished"
                )
                self._send_update(
                    session_id,
                    {
                        "sessionUpdate": "usage_update",
                        "used": 180,
                        "size": 1000000,
                    },
                )

            wakeup_thread = threading.Thread(target=_emit_wakeup, daemon=True)
            wakeup_thread.start()
            return

        with self._state_lock:
            is_first_turn = session["first_turn"]
            session["first_turn"] = False

        if is_first_turn:
            self._send_update(
                session_id,
                {
                    "sessionUpdate": "session_info_update",
                    "title": "Fake ACP Session",
                },
            )
        self._send_update(
            session_id,
            {
                "sessionUpdate": "agent_thought_chunk",
                "content": {"type": "text", "text": "Thinking through request"},
            },
        )
        self._send_text_chunk(session_id, "Hello ")
        self._send_text_chunk(session_id, "from Fake ACP")
        self._send_update(
            session_id,
            {
                "sessionUpdate": "usage_update",
                "used": 135,
                "size": 1000000,
            },
        )
        self._send_response(
            req_id,
            {
                "stopReason": "end_turn",
                "usage": {
                    "inputTokens": 100,
                    "outputTokens": 25,
                    "cachedReadTokens": 10,
                    "cachedWriteTokens": 0,
                    "thoughtTokens": 5,
                    "totalTokens": 135,
                },
            },
        )

    def _handle_cancel(self, params: dict[str, Any]) -> None:
        session_id = params.get("sessionId")
        if not isinstance(session_id, str):
            return
        with self._state_lock:
            session = self._sessions.get(session_id)
            cancel_event = session["cancel_event"] if session else None
        if cancel_event is not None:
            cancel_event.set()

    def _handle_session_close(self, req_id: Any, params: dict[str, Any]) -> None:
        session_id = params.get("sessionId")
        if isinstance(session_id, str):
            with self._state_lock:
                session = self._sessions.pop(session_id, None)
                if session is not None:
                    session["cancel_event"].set()
        self._send_response(req_id, {})

    def _handle_client_response(self, msg: dict[str, Any]) -> None:
        msg_id = msg.get("id")
        if msg_id is None:
            return
        key = str(msg_id)
        with self._state_lock:
            waiter = self._perm_waiters.pop(key, None)
        if waiter is not None:
            wait_event, holder = waiter
            holder["response"] = msg
            wait_event.set()

    def run(self) -> int:
        workers: list[threading.Thread] = []
        for raw_line in sys.stdin:
            stripped = raw_line.strip()
            if not stripped:
                continue
            self._log_raw(stripped)
            try:
                msg = json.loads(stripped)
            except json.JSONDecodeError as exc:
                self._send_error(None, -32700, f"Parse error: {exc}")
                continue

            if not isinstance(msg, dict):
                self._send_error(None, -32600, "Invalid Request")
                continue

            method = msg.get("method")
            req_id = msg.get("id")
            params = msg.get("params")
            if not isinstance(params, dict):
                params = {}

            if method is None:
                self._handle_client_response(msg)
                continue

            if method == "initialize":
                self._handle_initialize(req_id)
            elif method == "authenticate":
                self._handle_authenticate(req_id)
            elif method == "session/new":
                self._handle_session_new(req_id, params)
            elif method == "session/prompt":
                worker = threading.Thread(
                    target=self._run_prompt_turn,
                    args=(req_id, params),
                    daemon=True,
                )
                worker.start()
                workers.append(worker)
            elif method == "session/cancel":
                self._handle_cancel(params)
            elif method == "session/close":
                self._handle_session_close(req_id, params)
            elif req_id is not None:
                self._send_error(req_id, -32601, f"Method not found: {method}")

        self._shutdown_event.set()
        for worker in workers:
            worker.join(timeout=0.5)
        return 0


def parse_args(argv: list[str]) -> argparse.Namespace:
    parser = argparse.ArgumentParser(description="Fake ACP v1 NDJSON server")
    parser.add_argument("--log-file", dest="log_file", default=None)
    args, _ = parser.parse_known_args(argv)
    return args


def main() -> int:
    args = parse_args(sys.argv[1:])
    log_path = args.log_file or os.environ.get("FAKE_ACP_LOG")
    server = FakeAcpServer(log_path)
    return server.run()


if __name__ == "__main__":
    raise SystemExit(main())
