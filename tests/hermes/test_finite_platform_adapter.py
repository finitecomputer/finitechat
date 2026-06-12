import asyncio
import importlib.util
import os
import sys
import types
import unittest
from dataclasses import dataclass, field
from enum import Enum
from pathlib import Path
from typing import Any, Optional


REPO_ROOT = Path(__file__).resolve().parents[2]
ADAPTER_PATH = REPO_ROOT / "integrations" / "hermes" / "finite-platform" / "adapter.py"


class Platform(Enum):
    FINITE = "finite"


@dataclass
class PlatformConfig:
    enabled: bool = True
    extra: dict[str, Any] = field(default_factory=dict)


class MessageType(Enum):
    TEXT = "text"
    LOCATION = "location"
    PHOTO = "photo"
    VIDEO = "video"
    AUDIO = "audio"
    VOICE = "voice"
    DOCUMENT = "document"
    STICKER = "sticker"
    COMMAND = "command"


@dataclass
class MessageEvent:
    text: str
    message_type: MessageType = MessageType.TEXT
    source: Any = None
    raw_message: Any = None
    message_id: Optional[str] = None
    platform_update_id: Optional[int] = None
    media_urls: list[str] = field(default_factory=list)
    media_types: list[str] = field(default_factory=list)
    reply_to_message_id: Optional[str] = None
    reply_to_text: Optional[str] = None
    auto_skill: Any = None
    channel_prompt: Optional[str] = None
    internal: bool = False


@dataclass
class SendResult:
    success: bool
    message_id: Optional[str] = None
    error: Optional[str] = None
    raw_response: Any = None
    retryable: bool = False


class BasePlatformAdapter:
    def __init__(self, config: PlatformConfig, platform: Platform):
        self.config = config
        self.platform = platform
        self._connected = False
        self.handled_messages: list[MessageEvent] = []

    @property
    def is_connected(self):
        return self._connected

    def _mark_connected(self):
        self._connected = True

    def _mark_disconnected(self):
        self._connected = False

    async def cancel_background_tasks(self):
        return None

    def build_source(self, **kwargs):
        kwargs.setdefault("platform", self.platform)
        return types.SimpleNamespace(**kwargs)

    async def handle_message(self, event: MessageEvent) -> None:
        self.handled_messages.append(event)


def install_gateway_stubs() -> None:
    gateway = types.ModuleType("gateway")
    config = types.ModuleType("gateway.config")
    platforms = types.ModuleType("gateway.platforms")
    base = types.ModuleType("gateway.platforms.base")

    config.Platform = Platform
    config.PlatformConfig = PlatformConfig
    base.BasePlatformAdapter = BasePlatformAdapter
    base.MessageEvent = MessageEvent
    base.MessageType = MessageType
    base.SendResult = SendResult

    sys.modules["gateway"] = gateway
    sys.modules["gateway.config"] = config
    sys.modules["gateway.platforms"] = platforms
    sys.modules["gateway.platforms.base"] = base


def load_adapter_module():
    install_gateway_stubs()
    module_name = "finite_platform_adapter_under_test"
    sys.modules.pop(module_name, None)
    spec = importlib.util.spec_from_file_location(module_name, ADAPTER_PATH)
    module = importlib.util.module_from_spec(spec)
    assert spec.loader is not None
    sys.modules[module_name] = module
    spec.loader.exec_module(module)
    return module


class MockPluginContext:
    def __init__(self):
        self.registered: list[dict[str, Any]] = []

    def register_platform(self, **kwargs):
        self.registered.append(kwargs)


class FinitePlatformAdapterTests(unittest.TestCase):
    def setUp(self):
        self.module = load_adapter_module()

    def adapter(self, room_id="room-agent-1"):
        extra = {"home": "/tmp/finite-agent-home", "finitechat_bin": "/bin/echo"}
        if room_id:
            extra["room_id"] = room_id
        return self.module.FiniteChatAdapter(PlatformConfig(extra=extra))

    def test_register_exposes_finite_platform_contract(self):
        ctx = MockPluginContext()
        self.module.register(ctx)

        self.assertEqual(len(ctx.registered), 1)
        entry = ctx.registered[0]
        self.assertEqual(entry["name"], "finite")
        self.assertEqual(entry["label"], "Finite Chat")
        self.assertEqual(entry["required_env"], ["FINITECHAT_HOME"])
        self.assertEqual(entry["allowed_users_env"], "FINITECHAT_ALLOWED_USERS")
        self.assertEqual(entry["max_message_length"], self.module.FiniteChatAdapter.MAX_MESSAGE_LENGTH)
        self.assertTrue(callable(entry["adapter_factory"]))

    def test_check_requirements_uses_finitechat_bin_not_finitecomputer(self):
        old_value = os.environ.get("FINITECHAT_BIN")
        os.environ["FINITECHAT_BIN"] = "/bin/echo"
        try:
            self.assertTrue(self.module.check_requirements())
        finally:
            if old_value is None:
                os.environ.pop("FINITECHAT_BIN", None)
            else:
                os.environ["FINITECHAT_BIN"] = old_value

    def test_send_translates_hermes_room_thread_and_metadata_to_bridge_json(self):
        adapter = self.adapter()
        calls = []

        async def fake_json(action, payload, *, timeout):
            calls.append((action, payload, timeout))
            return self.module._FiniteChatResult(True, {"message_id": "out-1"}, None, False)

        adapter._finitechat_json = fake_json
        result = asyncio.run(
            adapter.send(
                "room-agent-1",
                "hello",
                reply_to="msg-0",
                metadata={"thread_id": "topic-build", "priority": "low"},
            )
        )

        self.assertTrue(result.success)
        self.assertEqual(result.message_id, "out-1")
        self.assertEqual(calls[0][0], "send")
        payload = calls[0][1]
        self.assertEqual(payload["room_id"], "room-agent-1")
        self.assertEqual(payload["conversation_id"], "topic-build")
        self.assertEqual(payload["reply_to_message_id"], "msg-0")
        self.assertEqual(payload["kind"], "message")
        self.assertEqual(payload["status"], "complete")
        self.assertEqual(payload["metadata"], {"priority": "low"})

    def test_media_send_uses_typed_attachment_payload(self):
        adapter = self.adapter()
        calls = []

        async def fake_json(action, payload, *, timeout):
            calls.append((action, payload, timeout))
            return self.module._FiniteChatResult(True, {"message_id": "media-1"}, None, False)

        adapter._finitechat_json = fake_json
        result = asyncio.run(
            adapter.send_document(
                "room-agent-1",
                "/tmp/report.pdf",
                caption="report",
                metadata={"thread_id": "topic-docs"},
            )
        )

        self.assertTrue(result.success)
        payload = calls[0][1]
        self.assertEqual(payload["conversation_id"], "topic-docs")
        self.assertEqual(payload["kind"], "media")
        self.assertEqual(payload["attachments"][0]["kind"], "file")
        self.assertEqual(payload["attachments"][0]["mime_type"], "application/pdf")

    def test_poll_event_maps_room_to_chat_and_conversation_to_thread(self):
        adapter = self.adapter()
        calls = []

        async def fake_json(action, payload, *, timeout):
            calls.append((action, payload, timeout))
            return self.module._FiniteChatResult(True, {}, None, False)

        adapter._finitechat_json = fake_json
        raw_event = {
            "room_id": "room-agent-1",
            "seq": 12,
            "message_id": "msg-12",
            "conversation_id": "topic-build",
            "text": "please build",
            "message_type": "text",
            "source": {
                "platform": "finite",
                "chat_id": "room-agent-1",
                "chat_type": "dm",
                "user_id": "alice",
                "user_name": "Alice",
                "thread_id": "topic-build",
                "chat_topic": "Builds",
            },
            "attachments": [
                {
                    "kind": "image",
                    "path": "/tmp/screenshot.png",
                    "name": "screenshot.png",
                    "mime_type": "image/png",
                }
            ],
            "reply_to_message_id": "msg-11",
            "reply_to_text": "previous",
            "auto_skill": "coding",
            "channel_prompt": "project prompt",
        }

        asyncio.run(adapter._handle_finitechat_event(raw_event))

        self.assertEqual(len(adapter.handled_messages), 1)
        event = adapter.handled_messages[0]
        self.assertEqual(event.text, "please build")
        self.assertEqual(event.message_type, MessageType.PHOTO)
        self.assertEqual(event.source.chat_id, "room-agent-1")
        self.assertEqual(event.source.thread_id, "topic-build")
        self.assertEqual(event.source.chat_topic, "Builds")
        self.assertEqual(event.media_urls, ["/tmp/screenshot.png"])
        self.assertEqual(event.reply_to_message_id, "msg-11")
        # The CLI owns the durable cursor; the adapter never acks.
        self.assertEqual(calls, [])

    def test_room_filter_drops_other_rooms_but_unfiltered_serves_all(self):
        filtered = self.adapter(room_id="room-agent-1")
        filtered._finitechat_json = self._record_json([])
        asyncio.run(
            filtered._handle_finitechat_event(
                {"room_id": "other-room", "seq": 1, "message_id": "msg-1", "text": "nope"}
            )
        )
        self.assertEqual(filtered.handled_messages, [])

        unfiltered = self.adapter(room_id=None)
        unfiltered._finitechat_json = self._record_json([])
        asyncio.run(
            unfiltered._handle_finitechat_event(
                {"room_id": "any-room", "seq": 2, "message_id": "msg-2", "text": "hello"}
            )
        )
        self.assertEqual(len(unfiltered.handled_messages), 1)
        self.assertEqual(unfiltered.handled_messages[0].source.chat_id, "any-room")

    def test_home_is_required_and_room_is_optional(self):
        self.assertTrue(
            self.module.validate_config(
                PlatformConfig(extra={"home": "/tmp/finite-agent-home"})
            )
        )
        old_home = os.environ.pop("FINITECHAT_HOME", None)
        try:
            self.assertFalse(self.module.validate_config(PlatformConfig(extra={})))
        finally:
            if old_home is not None:
                os.environ["FINITECHAT_HOME"] = old_home

    def test_connect_surfaces_invite_qr_url_and_pin(self):
        adapter = self.adapter(room_id=None)
        calls = []

        async def fake_json(action, payload, *, timeout):
            calls.append(action)
            if action == "pin":
                return self.module._FiniteChatResult(False, {}, "no stored invites", False)
            if action == "invite":
                return self.module._FiniteChatResult(
                    True,
                    {
                        "qr": "█▀▀▀█ qr █▀▀▀█",
                        "url": "finite://join?v=1&s=http%3A%2F%2Fx&r=r&i=i&t=00&a=npub1q",
                        "pin": "123456",
                        "pin_window_seconds": 30,
                    },
                    None,
                    False,
                )
            return self.module._FiniteChatResult(True, {}, None, False)

        adapter._finitechat_json = fake_json
        asyncio.run(adapter._surface_invite())
        # Falls back from the stored-invite lookup to creating one.
        self.assertEqual(calls, ["pin", "invite"])

    def _record_json(self, calls):
        async def fake_json(action, payload, *, timeout):
            calls.append((action, payload, timeout))
            return self.module._FiniteChatResult(True, {}, None, False)

        return fake_json

    def test_typing_activity_uses_ephemeral_bridge_not_status_messages(self):
        adapter = self.adapter()
        calls = []

        async def fake_json(action, payload, *, timeout):
            calls.append((action, payload, timeout))
            return self.module._FiniteChatResult(True, {}, None, False)

        adapter._finitechat_json = fake_json
        asyncio.run(adapter.send_typing("room-agent-1", metadata={"thread_id": "topic-build"}))
        asyncio.run(adapter.stop_typing("room-agent-1"))

        self.assertEqual(calls[0][0], "activity")
        self.assertEqual(calls[0][1]["action"], "set")
        self.assertEqual(calls[0][1]["conversation_id"], "topic-build")
        self.assertEqual(calls[0][1]["expires_in_millis"], 60 * 1000)
        self.assertEqual(calls[1][0], "activity")
        self.assertEqual(calls[1][1]["action"], "clear")
        self.assertEqual(calls[1][1]["conversation_id"], "topic-build")


if __name__ == "__main__":
    unittest.main()
