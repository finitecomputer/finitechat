"""Finite Chat platform plugin for Hermes.

The adapter is intentionally thin: Hermes callbacks become JSON bridge
requests, and the finitechat daemon/CLI owns validation, cursoring, storage,
encryption, and attachment materialization.
"""

from __future__ import annotations

import asyncio
import contextlib
import json
import logging
import os
import shlex
import shutil
import urllib.error
import urllib.parse
import urllib.request
from pathlib import Path
from typing import Any

from gateway.config import Platform, PlatformConfig
from gateway.platforms.base import BasePlatformAdapter, MessageEvent, MessageType, SendResult

logger = logging.getLogger(__name__)

FINITE_PLATFORM_NAME = "finite"
LOCAL_ENV_FILE = "finitechat.env"
DEFAULT_POLL_LIMIT = 10
DEFAULT_POLL_TIMEOUT_SECS = 20
DEFAULT_ACTIVITY_REFRESH_SECS = 30.0
ACTIVE_TURN_POLL_TIMEOUT_MILLIS = 1000
DEFAULT_SERVICE_ADDR = "127.0.0.1:0"
SERVICE_READY_FILE = "hermes-service.json"
SERVICE_START_TIMEOUT_SECS = 5.0
MAX_DELIVERED_EVENT_KEYS = 256
STREAM_RECONNECT_BACKOFF_SECS = 2.0


def _load_local_env_defaults(path: Path | None = None) -> None:
    env_path = path or Path(__file__).with_name(LOCAL_ENV_FILE)
    try:
        raw = env_path.read_text(encoding="utf-8")
    except FileNotFoundError:
        return
    except OSError as exc:
        logger.warning("[finite] could not read %s: %s", env_path, exc)
        return

    for line in raw.splitlines():
        stripped = line.strip()
        if not stripped or stripped.startswith("#") or "=" not in stripped:
            continue
        key, value = stripped.split("=", 1)
        key = key.strip()
        if not key.startswith("FINITECHAT_"):
            continue
        os.environ.setdefault(key, value.strip())


_load_local_env_defaults()


def check_requirements() -> bool:
    return bool(_resolve_finitechat_command(""))


def validate_config(config: PlatformConfig) -> bool:
    extra = getattr(config, "extra", {}) or {}
    return bool(extra.get("home") or os.getenv("FINITECHAT_HOME"))


def is_connected(config: PlatformConfig) -> bool:
    return validate_config(config) and check_requirements()


class FiniteChatAdapter(BasePlatformAdapter):
    """Poll Finite Chat for inbound messages and deliver Hermes replies."""

    MAX_MESSAGE_LENGTH = 12000
    SUPPORTS_MESSAGE_EDITING = True

    def __init__(self, config: PlatformConfig):
        super().__init__(config, _finite_platform())
        extra = getattr(config, "extra", {}) or {}
        self.home = str(extra.get("home") or os.getenv("FINITECHAT_HOME") or "").strip()
        # Optional room filter; by default the adapter serves every room the
        # agent's invites admit people into.
        self.room_id = str(extra.get("room_id") or os.getenv("FINITECHAT_ROOM_ID") or "").strip()
        self.poll_timeout_secs = _bounded_int(
            extra.get("poll_timeout_secs") or os.getenv("FINITECHAT_HERMES_POLL_TIMEOUT_SECS"),
            DEFAULT_POLL_TIMEOUT_SECS,
            minimum=1,
            maximum=60,
        )
        self.poll_limit = _bounded_int(
            extra.get("poll_limit") or os.getenv("FINITECHAT_HERMES_POLL_LIMIT"),
            DEFAULT_POLL_LIMIT,
            minimum=1,
            maximum=32,
        )
        self.activity_refresh_secs = float(
            _bounded_int(
                extra.get("activity_refresh_secs")
                or os.getenv("FINITECHAT_HERMES_ACTIVITY_REFRESH_SECS"),
                int(DEFAULT_ACTIVITY_REFRESH_SECS),
                minimum=5,
                maximum=120,
            )
        )
        self.service_url = (
            str(extra.get("service_url") or os.getenv("FINITECHAT_HERMES_SERVICE_URL") or "")
            .strip()
            .rstrip("/")
        )
        self.service_addr = str(
            extra.get("service_addr")
            or os.getenv("FINITECHAT_HERMES_SERVICE_ADDR")
            or DEFAULT_SERVICE_ADDR
        ).strip()
        self.inbound_stream = _bounded_bool(
            extra.get("inbound_stream")
            if "inbound_stream" in extra
            else os.getenv("FINITECHAT_HERMES_INBOUND_STREAM"),
            default=False,
        )
        self._poll_task: asyncio.Task | None = None
        self._service_proc: asyncio.subprocess.Process | None = None
        self._service_ready_file: Path | None = None
        self._finitechat_cmd = _resolve_finitechat_command(str(extra.get("finitechat_bin") or ""))
        self._finitechat_lock = asyncio.Lock()
        self._delivered_event_keys: set[str] = set()
        self._delivered_event_order: list[str] = []

    async def connect(self) -> bool:
        if not self.home:
            logger.error("[finite] FINITECHAT_HOME is required (agent home directory)")
            return False
        if not self._finitechat_cmd:
            logger.error("[finite] finitechat CLI is not configured")
            return False

        await self._ensure_service()
        await self._recover_interrupted_turns()
        await self._surface_invite()
        self._mark_connected()
        if self.inbound_stream and self.service_url:
            self._poll_task = asyncio.create_task(self._stream_loop())
        else:
            self._poll_task = asyncio.create_task(self._poll_loop())
        logger.info(
            "[finite] connected (home=%s%s%s)",
            self.home,
            f", room filter={self.room_id}" if self.room_id else "",
            ", inbound stream=on" if self.inbound_stream and self.service_url else "",
        )
        return True

    async def _surface_invite(self) -> None:
        """Print the join QR/URL/PIN at startup (the agentnoise pattern for
        headless boxes). Reuses the stored invite; creates one on first run."""
        result = await self._finitechat_json("pin", {}, timeout=30)
        if not result.ok:
            result = await self._finitechat_json("invite", {}, timeout=60)
        if not result.ok:
            logger.warning("[finite] could not prepare an invite: %s", result.error)
            return
        qr = result.data.get("qr") or ""
        url = result.data.get("url") or ""
        pin = result.data.get("pin") or ""
        if qr:
            print(qr, flush=True)
        if url:
            print(f"Scan or open in Finite Chat:\n  {url}", flush=True)
        if pin:
            print(
                f"Challenge PIN (rotates every "
                f"{result.data.get('pin_window_seconds') or 30}s): {pin}",
                flush=True,
            )

    async def _recover_interrupted_turns(self) -> None:
        result = await self._finitechat_json("recover", {}, timeout=60)
        if not result.ok:
            logger.warning("[finite] could not recover interrupted turns: %s", result.error)
            return
        recovered = result.data.get("recovered") or 0
        if recovered:
            logger.info("[finite] recovered %s interrupted Hermes turn(s)", recovered)

    async def disconnect(self) -> None:
        if self._poll_task:
            self._poll_task.cancel()
            with contextlib.suppress(asyncio.CancelledError):
                await self._poll_task
            self._poll_task = None
        await self._stop_service()
        await self.cancel_background_tasks()
        self._mark_disconnected()
        logger.info("[finite] disconnected")

    async def send(
        self,
        chat_id: str,
        content: str,
        reply_to: str | None = None,
        metadata: dict[str, Any] | None = None,
    ) -> SendResult:
        payload = self._send_payload(chat_id, content, reply_to, metadata)
        result = await self._finitechat_json("send", payload, timeout=30)
        if not result.ok:
            return SendResult(success=False, error=result.error, retryable=result.retryable)
        return SendResult(
            success=True,
            message_id=str(result.data.get("message_id") or result.data.get("id") or "") or None,
            raw_response=result.data,
        )

    async def edit_message(
        self,
        chat_id: str,
        message_id: str,
        content: str,
        *,
        finalize: bool = False,
    ) -> SendResult:
        payload = {
            "room_id": self._room_id(chat_id),
            "conversation_id": self._conversation_id(None),
            "message_id": str(message_id),
            "text": str(content),
            "status": "complete" if finalize else "running",
            "finalize": bool(finalize),
            "metadata": {},
        }
        result = await self._finitechat_json("edit", payload, timeout=30)
        if not result.ok:
            return SendResult(success=False, error=result.error, retryable=result.retryable)
        return SendResult(
            success=True,
            message_id=str(result.data.get("message_id") or message_id),
            raw_response=result.data,
        )

    async def send_typing(self, chat_id: str, metadata=None) -> None:
        payload = self._activity_payload(chat_id, metadata, action="set")
        await self._finitechat_json("activity", payload, timeout=15)

    async def stop_typing(self, chat_id: str) -> None:
        payload = self._activity_payload(chat_id, None, action="clear")
        await self._finitechat_json("activity", payload, timeout=15)

    async def _keep_typing(
        self,
        chat_id: str,
        interval: float = DEFAULT_ACTIVITY_REFRESH_SECS,
        metadata=None,
        stop_event: asyncio.Event | None = None,
    ) -> None:
        grace_deadline = asyncio.get_running_loop().time() + 0.75
        while asyncio.get_running_loop().time() < grace_deadline:
            if stop_event is not None and stop_event.is_set():
                return
            await asyncio.sleep(0.05)

        refresh_secs = self.activity_refresh_secs if self.activity_refresh_secs > 0 else interval
        while stop_event is None or not stop_event.is_set():
            await self.send_typing(chat_id, metadata=metadata)
            if stop_event is None:
                await asyncio.sleep(refresh_secs)
                continue
            try:
                await asyncio.wait_for(stop_event.wait(), timeout=refresh_secs)
            except TimeoutError:
                continue

    async def send_image(
        self,
        chat_id: str,
        image_url: str,
        caption: str | None = None,
        reply_to: str | None = None,
        metadata: dict[str, Any] | None = None,
    ) -> SendResult:
        return await self._send_media(
            chat_id,
            caption or "",
            {"kind": "image", "url": image_url, "name": caption or "image", "mime_type": "image/*"},
            reply_to=reply_to,
            metadata=metadata,
        )

    async def send_image_file(
        self,
        chat_id: str,
        image_path: str,
        caption: str | None = None,
        reply_to: str | None = None,
        metadata: dict[str, Any] | None = None,
    ) -> SendResult:
        return await self._send_media(
            chat_id,
            caption or "",
            _local_attachment(image_path, "image"),
            reply_to=reply_to,
            metadata=metadata,
        )

    async def send_video(
        self,
        chat_id: str,
        video_path: str,
        caption: str | None = None,
        reply_to: str | None = None,
        metadata: dict[str, Any] | None = None,
    ) -> SendResult:
        return await self._send_media(
            chat_id,
            caption or "",
            _local_attachment(video_path, "video"),
            reply_to=reply_to,
            metadata=metadata,
        )

    async def send_voice(
        self,
        chat_id: str,
        audio_path: str,
        metadata: dict[str, Any] | None = None,
    ) -> SendResult:
        return await self._send_media(
            chat_id,
            "",
            _local_attachment(audio_path, "audio"),
            metadata=metadata,
        )

    async def send_document(
        self,
        chat_id: str,
        file_path: str,
        caption: str | None = None,
        reply_to: str | None = None,
        metadata: dict[str, Any] | None = None,
    ) -> SendResult:
        return await self._send_media(
            chat_id,
            caption or "",
            _local_attachment(file_path, "file"),
            reply_to=reply_to,
            metadata=metadata,
        )

    @staticmethod
    def extract_local_files(content: str):
        return [], content

    async def get_chat_info(self, chat_id: str) -> dict[str, Any]:
        room_id = self._room_id(chat_id)
        return {"id": room_id, "name": room_id, "type": "finite"}

    async def _poll_loop(self) -> None:
        while self.is_connected:
            if not await self._poll_once():
                await asyncio.sleep(2.0)

    async def _poll_once(self) -> bool:
        result = await self._finitechat_json(
            "poll",
            self._inbound_request_payload(),
            timeout=self.poll_timeout_secs + 15,
        )
        if not result.ok:
            logger.warning("[finite] poll failed: %s", result.error)
            return False
        await self._process_poll_payload(result.data)
        return True

    async def _stream_loop(self) -> None:
        while self.is_connected:
            if not self.service_url and not await self._ensure_service():
                if not await self._poll_once():
                    await asyncio.sleep(STREAM_RECONNECT_BACKOFF_SECS)
                continue
            result = await asyncio.to_thread(
                _finitechat_service_inbound,
                self.service_url,
                self._inbound_request_payload(),
                self.poll_timeout_secs + 15,
            )
            if result.ok:
                await self._process_inbound_records(result.data.get("records") or [])
                continue
            logger.warning("[finite] inbound stream failed: %s", result.error)
            if result.transport_error:
                self.service_url = ""
            await asyncio.sleep(STREAM_RECONNECT_BACKOFF_SECS)

    def _inbound_request_payload(self) -> dict[str, Any]:
        timeout_millis = self.poll_timeout_secs * 1000
        if self._has_active_turn():
            timeout_millis = min(timeout_millis, ACTIVE_TURN_POLL_TIMEOUT_MILLIS)
        payload: dict[str, Any] = {
            "limit": self.poll_limit,
            "timeout_millis": timeout_millis,
        }
        if self.room_id:
            payload["room_id"] = self.room_id
        return payload

    async def _process_poll_payload(self, data: dict[str, Any]) -> None:
        for account in data.get("joined") or []:
            logger.info("[finite] verified joiner admitted: %s", account)
        for raw_event in data.get("events") or []:
            await self._dispatch_raw_event(raw_event)

    async def _process_inbound_records(self, records: list[Any]) -> None:
        for raw_record in records:
            if not isinstance(raw_record, dict):
                continue
            record_type = str(raw_record.get("type") or "")
            if record_type == "joined":
                logger.info("[finite] verified joiner admitted: %s", raw_record.get("account_id"))
                continue
            raw_event = raw_record.get("event") if record_type == "event" else raw_record
            await self._dispatch_raw_event(raw_event)

    async def _dispatch_raw_event(self, raw_event: Any) -> None:
        try:
            await self._handle_finitechat_event(raw_event)
        except Exception as exc:
            logger.error("[finite] failed to dispatch event: %s", exc, exc_info=True)

    async def _handle_finitechat_event(self, raw_event: dict[str, Any]) -> None:
        if not isinstance(raw_event, dict):
            return
        room_id = str(raw_event.get("room_id") or self.room_id)
        if self.room_id and room_id != self.room_id:
            logger.warning("[finite] ignored event for filtered room %s", room_id)
            return
        seq = raw_event.get("seq")
        message_id = str(raw_event.get("message_id") or "")
        if not message_id:
            logger.warning("[finite] ignored event without message_id")
            return
        event_key = _adapter_event_key(room_id, seq, message_id)
        if event_key and event_key in self._delivered_event_keys:
            await self._ack_finitechat_event(room_id, seq, message_id)
            return

        raw_source = raw_event.get("source")
        source_data: dict[str, Any] = raw_source if isinstance(raw_source, dict) else {}
        conversation_id = _string_or_none(raw_event.get("conversation_id"))
        raw_attachments = raw_event.get("attachments")
        attachments: list[Any] = raw_attachments if isinstance(raw_attachments, list) else []
        media_urls, media_types = _event_media(attachments)
        source = self.build_source(
            chat_id=str(source_data.get("chat_id") or room_id),
            chat_name=_string_or_none(source_data.get("chat_name")),
            chat_type=str(source_data.get("chat_type") or "dm"),
            user_id=_string_or_none(source_data.get("user_id")) or "finite-user",
            user_name=_string_or_none(source_data.get("user_name")) or "Finite user",
            thread_id=_string_or_none(source_data.get("thread_id")) or conversation_id,
            chat_topic=_string_or_none(source_data.get("chat_topic")),
            user_id_alt=_string_or_none(source_data.get("user_id_alt")),
            chat_id_alt=_string_or_none(source_data.get("chat_id_alt")),
            is_bot=bool(source_data.get("is_bot") or False),
        )
        event = MessageEvent(
            text=str(raw_event.get("text") or ""),
            message_type=_message_type(str(raw_event.get("message_type") or ""), media_types),
            source=source,
            raw_message=raw_event,
            message_id=message_id,
            platform_update_id=seq if isinstance(seq, int) else None,
            media_urls=media_urls,
            media_types=media_types,
            reply_to_message_id=_string_or_none(raw_event.get("reply_to_message_id")),
            reply_to_text=_string_or_none(raw_event.get("reply_to_text")),
            auto_skill=raw_event.get("auto_skill"),
            channel_prompt=_string_or_none(raw_event.get("channel_prompt")),
            internal=bool(raw_event.get("internal") or False),
        )
        await self.handle_message(event)
        if event_key:
            self._remember_delivered_event(event_key)
        await self._ack_finitechat_event(room_id, seq, message_id)

    async def _ack_finitechat_event(self, room_id: str, seq: Any, message_id: str) -> None:
        if not isinstance(seq, int):
            return
        ack = await self._finitechat_json(
            "ack",
            {"room_id": room_id, "seq": seq, "message_id": message_id},
            timeout=15,
        )
        if not ack.ok:
            logger.warning("[finite] failed to ack %s/%s: %s", room_id, seq, ack.error)

    def _remember_delivered_event(self, event_key: str) -> None:
        if event_key in self._delivered_event_keys:
            return
        self._delivered_event_keys.add(event_key)
        self._delivered_event_order.append(event_key)
        while len(self._delivered_event_order) > MAX_DELIVERED_EVENT_KEYS:
            evicted = self._delivered_event_order.pop(0)
            self._delivered_event_keys.discard(evicted)

    async def _send_media(
        self,
        chat_id: str,
        body: str,
        attachment: dict[str, Any],
        *,
        reply_to: str | None = None,
        metadata: dict[str, Any] | None = None,
    ) -> SendResult:
        meta = self._message_metadata(metadata)
        attachments = list(meta.get("attachments") or [])
        attachments.append(attachment)
        meta["attachments"] = attachments
        meta["_finitechat_kind"] = "media"
        return await self.send(chat_id=chat_id, content=body, reply_to=reply_to, metadata=meta)

    def _send_payload(
        self,
        chat_id: str,
        content: str,
        reply_to: str | None,
        metadata: dict[str, Any] | None,
    ) -> dict[str, Any]:
        meta = self._message_metadata(metadata)
        conversation_id = self._conversation_id(meta)
        attachments = meta.pop("attachments", [])
        explicit_kind = meta.pop("_finitechat_kind", None)
        explicit_status = meta.pop("_finitechat_status", None)
        kind = str(explicit_kind or ("media" if attachments else _infer_finitechat_kind(content)))
        status = str(explicit_status or _infer_finitechat_status(content))
        return {
            "room_id": self._room_id(chat_id),
            "conversation_id": conversation_id,
            "text": str(content),
            "kind": kind,
            "status": status,
            "attachments": attachments if isinstance(attachments, list) else [],
            "reply_to_message_id": reply_to,
            "metadata": meta,
        }

    def _activity_payload(
        self,
        chat_id: str,
        metadata: dict[str, Any] | None,
        *,
        action: str,
    ) -> dict[str, Any]:
        return {
            "room_id": self._room_id(chat_id),
            "conversation_id": None,
            "activity_kind": "working",
            "activity_id": None,
            "action": action,
            "payload": None,
            "expires_in_millis": 60 * 1000,
        }

    def _has_active_turn(self) -> bool:
        active_sessions = getattr(self, "_active_sessions", None)
        return bool(active_sessions)

    def _room_id(self, chat_id: str | None) -> str:
        return str(chat_id or self.room_id).strip() or self.room_id

    @staticmethod
    def _conversation_id(metadata: dict[str, Any] | None) -> str | None:
        if not isinstance(metadata, dict):
            return None
        return _string_or_none(metadata.pop("thread_id", None)) or _string_or_none(
            metadata.pop("conversation_id", None)
        )

    @staticmethod
    def _message_metadata(metadata: dict[str, Any] | None) -> dict[str, Any]:
        if isinstance(metadata, dict):
            return dict(metadata)
        return {}

    async def _finitechat_json(
        self,
        action: str,
        payload: dict[str, Any],
        *,
        timeout: int,
    ) -> _FiniteChatResult:
        if self._service_proc is not None and self._service_proc.returncode is not None:
            self.service_url = ""
            await self._ensure_service()
        if self.service_url:
            result = await asyncio.to_thread(
                _finitechat_service_json,
                self.service_url,
                action,
                payload,
                timeout,
            )
            if result.ok or not result.transport_error:
                return result
            logger.warning(
                "[finite] Hermes service unavailable (%s); falling back to finitechat CLI",
                result.error,
            )
        if not self._finitechat_cmd:
            return _FiniteChatResult(False, {}, "finitechat CLI is not configured", False)
        command = [*self._finitechat_cmd, "hermes"]
        if self.home:
            command += ["--home", self.home]
        command += [action, "--json"]
        try:
            stdin = json.dumps(payload, ensure_ascii=False).encode("utf-8") + b"\n"
            async with self._finitechat_lock:
                proc = await asyncio.create_subprocess_exec(
                    *command,
                    env=os.environ.copy(),
                    stdin=asyncio.subprocess.PIPE,
                    stdout=asyncio.subprocess.PIPE,
                    stderr=asyncio.subprocess.PIPE,
                )
                stdout, stderr = await asyncio.wait_for(proc.communicate(stdin), timeout=timeout)
        except TimeoutError:
            return _FiniteChatResult(False, {}, "finitechat timed out", True)
        except FileNotFoundError as exc:
            return _FiniteChatResult(False, {}, str(exc), False)
        except Exception as exc:
            return _FiniteChatResult(False, {}, str(exc), True)

        stdout_text = stdout.decode("utf-8", errors="replace").strip()
        stderr_text = stderr.decode("utf-8", errors="replace").strip()
        if proc.returncode != 0:
            return _FiniteChatResult(
                False,
                {},
                stderr_text or stdout_text or f"finitechat exited {proc.returncode}",
                _is_retryable_cli_error(stderr_text or stdout_text),
            )
        if not stdout_text:
            return _FiniteChatResult(True, {}, None, False)
        try:
            return _FiniteChatResult(True, json.loads(stdout_text), None, False)
        except json.JSONDecodeError as exc:
            try:
                return _FiniteChatResult(
                    True, json.loads(stdout_text.splitlines()[-1]), None, False
                )
            except json.JSONDecodeError:
                return _FiniteChatResult(
                    False, {}, f"finitechat returned invalid JSON: {exc}", False
                )

    async def _ensure_service(self) -> bool:
        if self.service_url:
            healthy = await asyncio.to_thread(_finitechat_service_health, self.service_url, 2)
            if healthy:
                return True
            if self._service_proc is None:
                return False
            self.service_url = ""
        if self._service_proc is not None and self._service_proc.returncode is None:
            if self._service_ready_file is not None:
                started = _read_service_ready_file(self._service_ready_file)
                if started.get("url"):
                    self.service_url = str(started["url"]).rstrip("/")
                    logger.info("[finite] Hermes service ready at %s", self.service_url)
                    return True
            return bool(self.service_url)
        if not self.home or not self._finitechat_cmd:
            return False

        ready_file = Path(self.home) / SERVICE_READY_FILE
        self._service_ready_file = ready_file
        with contextlib.suppress(FileNotFoundError):
            ready_file.unlink()
        command = [
            *self._finitechat_cmd,
            "hermes",
            "--home",
            self.home,
            "serve",
            "--addr",
            self.service_addr,
            "--ready-file",
            str(ready_file),
            "--json",
        ]
        try:
            self._service_proc = await asyncio.create_subprocess_exec(
                *command,
                env=os.environ.copy(),
                stdout=asyncio.subprocess.DEVNULL,
                stderr=asyncio.subprocess.DEVNULL,
            )
        except Exception as exc:
            logger.warning("[finite] could not start Hermes service: %s", exc)
            return False

        deadline = asyncio.get_running_loop().time() + SERVICE_START_TIMEOUT_SECS
        while asyncio.get_running_loop().time() < deadline:
            if self._service_proc.returncode is not None:
                logger.warning(
                    "[finite] Hermes service exited during startup (%s)",
                    self._service_proc.returncode,
                )
                self._service_proc = None
                self._service_ready_file = None
                return False
            started = _read_service_ready_file(ready_file)
            if started.get("url"):
                self.service_url = str(started["url"]).rstrip("/")
                logger.info("[finite] Hermes service ready at %s", self.service_url)
                return True
            await asyncio.sleep(0.05)

        logger.warning("[finite] Hermes service did not become ready; using CLI bridge")
        return False

    async def _stop_service(self) -> None:
        proc = self._service_proc
        self._service_proc = None
        self._service_ready_file = None
        if proc is None or proc.returncode is not None:
            return
        proc.terminate()
        try:
            await asyncio.wait_for(proc.wait(), timeout=2.0)
        except TimeoutError:
            proc.kill()
            await proc.wait()


class _FiniteChatResult:
    def __init__(
        self,
        ok: bool,
        data: dict[str, Any],
        error: str | None,
        retryable: bool,
        transport_error: bool = False,
    ):
        self.ok = ok
        self.data = data
        self.error = error
        self.retryable = retryable
        self.transport_error = transport_error


def _resolve_finitechat_command(configured: str) -> list[str]:
    raw = str(
        configured or os.getenv("FINITECHAT_HERMES_BIN") or os.getenv("FINITECHAT_BIN") or ""
    ).strip()
    if raw:
        return shlex.split(raw)
    for name in ("finitechat",):
        path = shutil.which(name)
        if path:
            return [path]
    return []


def _finitechat_service_json(
    service_url: str,
    action: str,
    payload: dict[str, Any],
    timeout: int,
) -> _FiniteChatResult:
    encoded_action = urllib.parse.quote(str(action), safe="")
    request = urllib.request.Request(
        f"{service_url}/v1/hermes/{encoded_action}",
        data=json.dumps(payload, ensure_ascii=False).encode("utf-8"),
        headers={"Accept": "application/json", "Content-Type": "application/json"},
        method="POST",
    )
    try:
        with urllib.request.urlopen(request, timeout=timeout) as response:
            body = response.read().decode("utf-8", errors="replace").strip()
    except urllib.error.HTTPError as exc:
        body = exc.read().decode("utf-8", errors="replace").strip()
        return _FiniteChatResult(
            False,
            {},
            _service_error_body(body) or f"finitechat service returned HTTP {exc.code}",
            _is_retryable_cli_error(body),
            False,
        )
    except TimeoutError as exc:
        return _FiniteChatResult(False, {}, str(exc), True, False)
    except (urllib.error.URLError, OSError) as exc:
        return _FiniteChatResult(False, {}, str(exc), True, True)

    if not body:
        return _FiniteChatResult(True, {}, None, False)
    try:
        return _FiniteChatResult(True, json.loads(body), None, False)
    except json.JSONDecodeError as exc:
        return _FiniteChatResult(
            False, {}, f"finitechat service returned invalid JSON: {exc}", False
        )


def _finitechat_service_inbound(
    service_url: str,
    payload: dict[str, Any],
    timeout: int,
) -> _FiniteChatResult:
    query = urllib.parse.urlencode(
        {key: value for key, value in payload.items() if value is not None and value != ""}
    )
    url = f"{service_url}/v1/hermes/inbound"
    if query:
        url = f"{url}?{query}"
    request = urllib.request.Request(
        url,
        headers={"Accept": "application/x-ndjson, application/json"},
        method="GET",
    )
    try:
        with urllib.request.urlopen(request, timeout=timeout) as response:
            body = response.read().decode("utf-8", errors="replace")
    except urllib.error.HTTPError as exc:
        body = exc.read().decode("utf-8", errors="replace").strip()
        return _FiniteChatResult(
            False,
            {},
            _service_error_body(body) or f"finitechat service returned HTTP {exc.code}",
            _is_retryable_cli_error(body),
            False,
        )
    except TimeoutError as exc:
        return _FiniteChatResult(False, {}, str(exc), True, False)
    except (urllib.error.URLError, OSError) as exc:
        return _FiniteChatResult(False, {}, str(exc), True, True)

    records: list[Any] = []
    for line in body.splitlines():
        stripped = line.strip()
        if not stripped:
            continue
        try:
            record = json.loads(stripped)
        except json.JSONDecodeError as exc:
            return _FiniteChatResult(
                False,
                {},
                f"finitechat inbound stream returned invalid JSON: {exc}",
                False,
            )
        records.append(record)
    return _FiniteChatResult(True, {"records": records}, None, False)


def _service_error_body(body: str) -> str | None:
    try:
        data = json.loads(body)
    except json.JSONDecodeError:
        return body or None
    if isinstance(data, dict):
        error = data.get("error")
        if error:
            return str(error)
    return body or None


def _finitechat_service_health(service_url: str, timeout: int) -> bool:
    try:
        request = urllib.request.Request(
            f"{service_url.rstrip('/')}/healthz",
            headers={"Accept": "application/json"},
            method="GET",
        )
        with urllib.request.urlopen(request, timeout=timeout) as response:
            data = json.loads(response.read().decode("utf-8", errors="replace"))
    except Exception:
        return False
    return isinstance(data, dict) and data.get("status") == "ok"


def _adapter_event_key(room_id: str, seq: Any, message_id: str) -> str | None:
    if not isinstance(seq, int):
        return None
    return f"{room_id}\x1f{seq}\x1f{message_id}"


def _read_service_ready_file(path: Path) -> dict[str, Any]:
    try:
        raw = path.read_text(encoding="utf-8")
    except FileNotFoundError:
        return {}
    except OSError as exc:
        logger.warning("[finite] could not read Hermes service ready file %s: %s", path, exc)
        return {}
    try:
        data = json.loads(raw)
    except json.JSONDecodeError:
        return {}
    return data if isinstance(data, dict) else {}


def _event_media(attachments: list[Any]) -> tuple[list[str], list[str]]:
    urls: list[str] = []
    types: list[str] = []
    for item in attachments:
        if not isinstance(item, dict):
            continue
        media_ref = _string_or_none(item.get("path")) or _string_or_none(item.get("url"))
        if not media_ref:
            continue
        urls.append(media_ref)
        types.append(
            _string_or_none(item.get("mime_type"))
            or _string_or_none(item.get("mimeType"))
            or "application/octet-stream"
        )
    return urls, types


def _message_type(raw: str, media_types: list[str]) -> MessageType:
    value = raw.strip()
    if value == "command":
        return MessageType.COMMAND
    if value == "sticker":
        return MessageType.STICKER
    if value == "location":
        return MessageType.LOCATION
    if not media_types:
        return MessageType.TEXT
    first = media_types[0]
    if first.startswith("image/"):
        return MessageType.PHOTO
    if first.startswith("video/"):
        return MessageType.VIDEO
    if first.startswith("audio/"):
        return MessageType.AUDIO
    return MessageType.DOCUMENT


def _infer_finitechat_kind(content: str) -> str:
    text = str(content or "").strip()
    if not text:
        return "message"
    if text == "Hermes is working":
        return "status"
    first_line = text.splitlines()[0].lstrip()
    if first_line.startswith(("⚙", "🔧", "🛠", "🔍", "🔎", "📖", "💻", "🌐", "⚡")):
        return "tool"
    return "message"


def _infer_finitechat_status(content: str) -> str:
    return "running" if "▉" in str(content or "") else "complete"


def _local_attachment(path: str, kind: str) -> dict[str, Any]:
    local_path = Path(path)
    return {
        "kind": kind,
        "path": str(local_path),
        "name": local_path.name or kind,
        "mime_type": _mime_type_for_path(local_path),
    }


def _mime_type_for_path(path: Path) -> str:
    suffix = path.suffix.lower()
    return {
        ".png": "image/png",
        ".jpg": "image/jpeg",
        ".jpeg": "image/jpeg",
        ".webp": "image/webp",
        ".gif": "image/gif",
        ".svg": "image/svg+xml",
        ".mp3": "audio/mpeg",
        ".wav": "audio/wav",
        ".ogg": "audio/ogg",
        ".opus": "audio/ogg",
        ".mp4": "video/mp4",
        ".mov": "video/quicktime",
        ".webm": "video/webm",
        ".pdf": "application/pdf",
        ".txt": "text/plain",
        ".md": "text/markdown",
        ".json": "application/json",
    }.get(suffix, "application/octet-stream")


def _string_or_none(value: Any) -> str | None:
    if value is None:
        return None
    text = str(value).strip()
    return text or None


def _bounded_int(value: Any, default: int, *, minimum: int, maximum: int) -> int:
    try:
        parsed = int(value)
    except (TypeError, ValueError):
        parsed = default
    return max(minimum, min(maximum, parsed))


def _bounded_bool(value: Any, *, default: bool) -> bool:
    if value is None:
        return default
    if isinstance(value, bool):
        return value
    text = str(value).strip().lower()
    if not text:
        return default
    if text in {"1", "true", "yes", "on"}:
        return True
    if text in {"0", "false", "no", "off"}:
        return False
    return default


def _is_retryable_cli_error(message: str) -> bool:
    lowered = message.lower()
    return any(token in lowered for token in ("timed out", "connection", "temporarily", "busy"))


def _finite_platform() -> Platform:
    try:
        return Platform(FINITE_PLATFORM_NAME)
    except ValueError:
        return Platform.LOCAL


def register(ctx) -> None:
    ctx.register_platform(
        name=FINITE_PLATFORM_NAME,
        label="Finite Chat",
        adapter_factory=lambda cfg: FiniteChatAdapter(cfg),
        check_fn=check_requirements,
        validate_config=validate_config,
        is_connected=is_connected,
        required_env=["FINITECHAT_HOME"],
        install_hint=(
            "Install the finitechat binary, run `finitechat hermes "
            "init --server URL`, then install this plugin with "
            "`finitechat hermes install`."
        ),
        allowed_users_env="FINITECHAT_ALLOWED_USERS",
        allow_all_env="FINITECHAT_ALLOW_ALL_USERS",
        max_message_length=FiniteChatAdapter.MAX_MESSAGE_LENGTH,
        allow_update_command=True,
        platform_hint=(
            "You are chatting through Finite Chat. The room is the delivery "
            "boundary and the thread is the conversation/topic. Use normal "
            "markdown and native attachments when available."
        ),
    )
