import { FormEvent, KeyboardEvent, useCallback, useEffect, useMemo, useRef, useState } from "react";
import ReactMarkdown from "react-markdown";
import remarkGfm from "remark-gfm";
import {
  BotIcon,
  CheckIcon,
  ChevronRightIcon,
  CopyIcon,
  KeyRoundIcon,
  LinkIcon,
  Loader2Icon,
  MessageCircleIcon,
  MoreHorizontalIcon,
  PaperclipIcon,
  PlusIcon,
  RefreshCwIcon,
  SendIcon,
  ShieldCheckIcon,
  SparklesIcon,
  SquarePenIcon,
  XIcon,
} from "lucide-react";
import { FiniteBrand } from "@/components/finite-brand";
import {
  AppRoomMemberSummary,
  AppRoomSummary,
  AppState,
  AppTopicSummary,
  ChatMessage,
  dispatch,
  getState,
  resolveDaemonUrl,
  subscribeToUpdates,
} from "./daemon";
import { LegacyFiniteChatSourceMarker } from "./legacy/LegacyFiniteChatSourceMarker";

type DesktopIdentityStatus = {
  secureStorageAvailable: boolean;
  hasStoredAccountSecret: boolean;
};

type DesktopOnboardingStatus = {
  completed: boolean;
};

export function App() {
  const [daemonUrl, setDaemonUrl] = useState<string | null>(null);
  const [state, setState] = useState<AppState | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [composer, setComposer] = useState("");
  const [inviteUrl, setInviteUrl] = useState("");
  const [roomTitle, setRoomTitle] = useState("");
  const [newRoomOpen, setNewRoomOpen] = useState(false);
  const [copiedInvite, setCopiedInvite] = useState(false);
  const [accountMenuOpen, setAccountMenuOpen] = useState(false);
  const [identityStatus, setIdentityStatus] = useState<DesktopIdentityStatus | null>(null);
  const [onboardingStatus, setOnboardingStatus] = useState<DesktopOnboardingStatus | null>(null);
  const [identitySecret, setIdentitySecret] = useState("");
  const [identityBusy, setIdentityBusy] = useState(false);
  const inviteInputRef = useRef<HTMLInputElement | null>(null);
  const transcriptRef = useRef<HTMLElement | null>(null);

  useEffect(() => {
    let cancelled = false;
    resolveDaemonUrl()
      .then((url) => {
        if (!cancelled) {
          setDaemonUrl(url);
        }
      })
      .catch((reason: unknown) => {
        if (!cancelled) {
          setError(errorMessage(reason));
        }
      });
    return () => {
      cancelled = true;
    };
  }, []);

  const refresh = useCallback(async () => {
    if (!daemonUrl) {
      return;
    }
    setBusy(true);
    setError(null);
    try {
      setState(await getState(daemonUrl));
    } catch (reason) {
      setError(errorMessage(reason));
    } finally {
      setBusy(false);
    }
  }, [daemonUrl]);

  useEffect(() => {
    if (!daemonUrl) {
      return;
    }
    void refresh();
    return subscribeToUpdates(
      daemonUrl,
      (next) => {
        setError(null);
        setState(next);
      },
      (reason) => setError(reason.message)
    );
  }, [daemonUrl, refresh]);

  useEffect(() => {
    if (!daemonUrl || state) {
      return;
    }
    const timer = window.setInterval(() => void refresh(), 1200);
    return () => window.clearInterval(timer);
  }, [daemonUrl, refresh, state]);

  const run = useCallback(
    async (action: Parameters<typeof dispatch>[1]) => {
      if (!daemonUrl) {
        return null;
      }
      setBusy(true);
      setError(null);
      try {
        const next = await dispatch(daemonUrl, action);
        setState(next);
        return next;
      } catch (reason) {
        setError(errorMessage(reason));
        return null;
      } finally {
        setBusy(false);
      }
    },
    [daemonUrl]
  );

  const loadDesktopState = useCallback(async () => {
    if (!window.finiteChatDesktop) {
      setOnboardingStatus({ completed: true });
      return;
    }
    const [identity, onboarding] = await Promise.all([
      window.finiteChatDesktop.identityStatus(),
      window.finiteChatDesktop.onboardingStatus(),
    ]);
    setIdentityStatus(identity);
    setOnboardingStatus(onboarding);
  }, []);

  useEffect(() => {
    void loadDesktopState();
  }, [loadDesktopState]);

  useEffect(() => {
    if (!window.finiteChatDesktop) {
      return;
    }
    return window.finiteChatDesktop.onInviteUrl((url) => {
      setInviteUrl(url);
      void run({ ScanTarget: { value: url } });
    });
  }, [run]);

  const selectedRoom = useMemo(
    () => state?.rooms.find((room) => room.room_id === state.selected_room_id) ?? null,
    [state]
  );
  const selectedTopic = useMemo(
    () =>
      state?.topics.find(
        (topic) => topic.room_id === state.selected_room_id && topic.topic_id === state.selected_topic_id
      ) ?? null,
    [state]
  );
  const visibleTopics = useMemo(
    () => state?.topics.filter((topic) => topic.room_id === state.selected_room_id && !topic.archived) ?? [],
    [state]
  );
  const activeInvite =
    selectedRoom && state?.active_invite?.room_id === selectedRoom.room_id ? state.active_invite : null;
  const pendingInviteRoomId = state?.flow.invite_join_submission_room_id ?? null;
  const selectedMessages = state?.messages ?? [];
  const selectedMembers = selectedRoom?.members ?? [];
  const agentRooms = state?.rooms.filter((room) => room.is_agent_chat) ?? [];
  const selectedRoomHasCounterparty = selectedRoom
    ? selectedRoom.is_agent_chat || selectedMembers.some((member) => !member.current_device)
    : false;
  const canSendToSelectedRoom =
    Boolean(selectedRoom) && selectedRoom?.state === "Connected" && selectedRoomHasCounterparty;
  const selectedAgentRoom = selectedRoom?.is_agent_chat ? selectedRoom : agentRooms[0] ?? null;
  const selectedRoomNeedsAgent = Boolean(selectedRoom) && !selectedRoomHasCounterparty;
  const statusText = state ? (error ?? state.flow.notice_text ?? state.toast ?? state.status) : "starting daemon";
  const shortAccount = state?.identity.account_id ? shortId(state.identity.account_id) : "not connected";
  const showOnboarding = window.finiteChatDesktop ? onboardingStatus?.completed === false : false;

  useEffect(() => {
    const transcript = transcriptRef.current;
    if (!transcript) {
      return;
    }
    transcript.scrollTo({ top: transcript.scrollHeight, behavior: "smooth" });
  }, [selectedMessages.length, selectedRoom?.room_id, selectedTopic?.topic_id]);

  async function submitComposer(event: FormEvent) {
    event.preventDefault();
    const text = composer.trim();
    if (!text || !state) {
      return;
    }
    if (!selectedRoom) {
      setError(agentRooms.length > 0 ? "Select an agent chat before sending." : "Connect Hermes before sending.");
      inviteInputRef.current?.focus();
      return;
    }
    if (!canSendToSelectedRoom) {
      setError(
        selectedRoom.state === "Connected"
          ? "This chat has no other member. Connect Hermes before sending."
          : "This chat is not ready for messages yet."
      );
      inviteInputRef.current?.focus();
      return;
    }
    setComposer("");
    if (selectedTopic) {
      await run({
        SendTopicMessage: {
          room_id: selectedTopic.room_id,
          topic_id: selectedTopic.topic_id,
          text,
        },
      });
    } else {
      await run({ SendMessage: { room_id: selectedRoom.room_id, text } });
    }
  }

  async function submitInvite(event: FormEvent) {
    event.preventDefault();
    const value = inviteUrl.trim();
    if (value) {
      await run({ ScanTarget: { value } });
    }
  }

  async function createRoom(event?: FormEvent) {
    event?.preventDefault();
    if (!selectedAgentRoom) {
      setNewRoomOpen(false);
      setRoomTitle("");
      setError("Connect Hermes before starting a new chat.");
      inviteInputRef.current?.focus();
      return;
    }
    const displayName = roomTitle.trim() || "New chat";
    setRoomTitle("");
    setNewRoomOpen(false);
    await run({ CreateTopic: { room_id: selectedAgentRoom.room_id, title: displayName } });
  }

  async function syncRuntime() {
    await run({ StartRuntime: null });
  }

  async function createInvite() {
    if (!selectedRoom) {
      return;
    }
    await run({ CreateInvite: { room_id: selectedRoom.room_id } });
  }

  async function copyInvite() {
    if (!activeInvite) {
      return;
    }
    await navigator.clipboard.writeText(activeInvite.invite_url);
    setCopiedInvite(true);
    window.setTimeout(() => setCopiedInvite(false), 1400);
  }

  async function submitPendingInviteJoin() {
    if (!pendingInviteRoomId) {
      return;
    }
    await run({ SubmitInviteJoin: { pending_room_id: pendingInviteRoomId } });
  }

  async function finishOnboarding() {
    if (window.finiteChatDesktop) {
      setOnboardingStatus(await window.finiteChatDesktop.completeOnboarding());
    } else {
      setOnboardingStatus({ completed: true });
    }
    void refresh();
  }

  async function importDesktopIdentity(secret: string) {
    if (!window.finiteChatDesktop || !secret.trim()) {
      return;
    }
    setIdentityBusy(true);
    setError(null);
    try {
      setIdentityStatus(await window.finiteChatDesktop.importAccountSecret(secret));
      setIdentitySecret("");
      setAccountMenuOpen(false);
      setState(null);
      if (window.finiteChatDesktop) {
        setOnboardingStatus(await window.finiteChatDesktop.completeOnboarding());
      }
      window.setTimeout(() => void refresh(), 700);
    } catch (reason) {
      setError(errorMessage(reason));
    } finally {
      setIdentityBusy(false);
    }
  }

  async function submitAccountImport(event: FormEvent) {
    event.preventDefault();
    await importDesktopIdentity(identitySecret);
  }

  async function clearDesktopIdentity() {
    if (!window.finiteChatDesktop) {
      return;
    }
    setIdentityBusy(true);
    setError(null);
    try {
      setIdentityStatus(await window.finiteChatDesktop.clearAccountSecret());
      setState(null);
      window.setTimeout(() => void refresh(), 700);
    } catch (reason) {
      setError(errorMessage(reason));
    } finally {
      setIdentityBusy(false);
    }
  }

  return (
    <div className="finite-chat finite-chat--electron">
      <LegacyFiniteChatSourceMarker />
      <aside className="finite-chat__sidebar">
        <div className="finite-chat__sidebar-top">
          <div className="finite-chat__brand">
            <FiniteBrand />
          </div>
          <button
            type="button"
            className="ocean-icon-button"
            aria-label="Refresh"
            onClick={() => void syncRuntime()}
            disabled={busy}
          >
            {busy ? <Loader2Icon className="finite-chat__spin" aria-hidden /> : <RefreshCwIcon aria-hidden />}
          </button>
        </div>

        <nav className="finite-chat__sidebar-nav" aria-label="Chat navigation">
          <details className="finite-chat__folder finite-chat__folder--flat" open>
            <summary className="finite-chat__folder-summary">
              <span className="finite-chat__folder-main">
                <span className="finite-chat__folder-icon" aria-hidden>
                  <BotIcon />
                </span>
                <span className="finite-chat__folder-label">Hermes</span>
              </span>
              <ChevronRightIcon className="finite-chat__folder-chevron" aria-hidden />
            </summary>

            <div className="finite-chat__folder-body">
              {(state?.rooms ?? []).map((room) => (
                <RoomThreadButton
                  key={room.room_id}
                  room={room}
                  active={room.room_id === state?.selected_room_id}
                  onClick={() => void run({ OpenRoom: { room_id: room.room_id } })}
                />
              ))}
              {state && state.rooms.length === 0 ? (
                <div className="finite-chat__thread-empty">
                  <MessageCircleIcon aria-hidden />
                  <span>No chats</span>
                </div>
              ) : null}
            </div>
          </details>

          {newRoomOpen ? (
            <form className="finite-chat__sidebar-create" onSubmit={createRoom}>
              <input
                value={roomTitle}
                onChange={(event) => setRoomTitle(event.target.value)}
                placeholder={selectedAgentRoom ? "Topic name" : "Connect Hermes first"}
                autoFocus
                disabled={!selectedAgentRoom}
              />
              <button type="submit" className="ocean-icon-button" aria-label="Create chat" disabled={busy || !selectedAgentRoom}>
                <CheckIcon aria-hidden />
              </button>
              <button
                type="button"
                className="ocean-icon-button"
                aria-label="Cancel"
                onClick={() => {
                  setNewRoomOpen(false);
                  setRoomTitle("");
                }}
              >
                <XIcon aria-hidden />
              </button>
            </form>
          ) : null}

          <form className="finite-chat__invite-form" onSubmit={submitInvite}>
            <input
              ref={inviteInputRef}
              value={inviteUrl}
              onChange={(event) => setInviteUrl(event.target.value)}
              placeholder="finite://join..."
            />
            <button type="submit" className="ocean-icon-button" aria-label="Open invite" disabled={!inviteUrl.trim() || busy}>
              <LinkIcon aria-hidden />
            </button>
          </form>
        </nav>

        <button
          type="button"
          className="finite-chat__sidebar-new-chat-fab"
          aria-label={selectedAgentRoom ? "New chat" : "Connect agent"}
          disabled={busy}
          onClick={() => {
            if (selectedAgentRoom) {
              setNewRoomOpen(true);
            } else {
              inviteInputRef.current?.focus();
            }
          }}
        >
          {selectedAgentRoom ? <PlusIcon aria-hidden /> : <LinkIcon aria-hidden />}
          <span>{selectedAgentRoom ? "New chat" : "Connect agent"}</span>
        </button>

        <div className="finite-chat__sidebar-footer">
          {accountMenuOpen ? (
            <div className="finite-chat__account-menu">
              <div className="finite-chat__account-heading">
                <KeyRoundIcon aria-hidden />
                <span>{identityStatus?.hasStoredAccountSecret ? "Imported account" : "Local identity"}</span>
              </div>
              <div className="finite-chat__account-id">{shortAccount}</div>
              <form className="finite-chat__account-import" onSubmit={submitAccountImport}>
                <input
                  value={identitySecret}
                  onChange={(event) => setIdentitySecret(event.target.value)}
                  placeholder="nsec or 64-char secret"
                  type="password"
                  disabled={identityBusy || identityStatus?.secureStorageAvailable === false}
                />
                <button
                  type="submit"
                  className="finite-chat__command-button"
                  disabled={!identitySecret.trim() || identityBusy || identityStatus?.secureStorageAvailable === false}
                >
                  Save
                </button>
              </form>
              {identityStatus?.hasStoredAccountSecret ? (
                <button
                  className="finite-chat__account-link"
                  type="button"
                  onClick={() => void clearDesktopIdentity()}
                  disabled={identityBusy}
                >
                  Use local identity
                </button>
              ) : null}
              {identityStatus?.secureStorageAvailable === false ? (
                <div className="finite-chat__account-warning">Secure store unavailable</div>
              ) : null}
            </div>
          ) : null}
          <button type="button" className="finite-chat__user-row" onClick={() => setAccountMenuOpen((open) => !open)}>
            <span className="finite-chat__avatar" aria-hidden>
              {initials(state?.identity.device_id ?? "Desktop")}
            </span>
            <span className="finite-chat__user-name">{state?.identity.device_id ?? "Desktop"}</span>
            <MoreHorizontalIcon aria-hidden />
          </button>
        </div>
      </aside>

      <section className="finite-chat__workspace">
        <header className="finite-chat__topbar">
          <div className="finite-chat__identity">
            <strong>{selectedTopic?.title ?? selectedRoom?.display_name ?? "Finite Chat"}</strong>
            <span>
              <span className={`finite-chat__status-dot ${error ? "is-error" : state ? "is-running" : ""}`} aria-hidden />
              {selectedRoom ? selectedRoom.user_status_text || selectedRoom.state : statusText}
            </span>
          </div>
          <div className="finite-chat__topbar-actions">
            {state?.flow.scan_in_flight ? <Loader2Icon className="finite-chat__spin" aria-hidden /> : null}
            {selectedMembers.length > 0 ? <MembersPill members={selectedMembers} /> : null}
            {selectedRoom ? (
              <button
                type="button"
                className="ocean-icon-button"
                aria-label="Create invite"
                onClick={() => void createInvite()}
                disabled={busy || selectedRoom.can_create_invite === false}
              >
                <LinkIcon aria-hidden />
              </button>
            ) : null}
          </div>
        </header>

        {pendingInviteRoomId ? (
          <section className="finite-chat__notice finite-chat__notice--inline">
            <strong>Invite found</strong>
            <span>{pendingInviteRoomId}</span>
            <button type="button" className="finite-chat__command-button" onClick={() => void submitPendingInviteJoin()} disabled={busy}>
              Join
            </button>
          </section>
        ) : null}

        {activeInvite ? (
          <section className="finite-chat__notice finite-chat__notice--inline">
            <strong>Invite ready</strong>
            <span>{activeInvite.invite_url}</span>
            <button type="button" className="finite-chat__command-button" onClick={() => void copyInvite()} disabled={busy}>
              {copiedInvite ? <CheckIcon aria-hidden /> : <CopyIcon aria-hidden />}
              {copiedInvite ? "Copied" : "Copy"}
            </button>
          </section>
        ) : null}

        {error ? (
          <section className="finite-chat__notice finite-chat__notice--inline is-error">
            <strong>Daemon</strong>
            <span>{error}</span>
          </section>
        ) : null}

        {selectedRoom && visibleTopics.length > 0 ? (
          <TopicStrip
            topics={visibleTopics}
            selectedTopic={selectedTopic}
            selectedRoom={selectedRoom}
            onOpenRoom={() => void run({ OpenRoom: { room_id: selectedRoom.room_id } })}
            onOpenTopic={(topic) => void run({ OpenTopic: { room_id: topic.room_id, topic_id: topic.topic_id } })}
          />
        ) : null}

        {state && (!selectedRoom || selectedRoomNeedsAgent || selectedRoom.state !== "Connected") ? (
          <AgentConnectionPanel
            busy={busy}
            inviteUrl={inviteUrl}
            pendingInviteRoomId={pendingInviteRoomId}
            selectedRoom={selectedRoom}
            hasAgentRoom={agentRooms.length > 0}
            onInviteUrlChange={setInviteUrl}
            onSubmitInvite={submitInvite}
            onSubmitPendingInviteJoin={submitPendingInviteJoin}
          />
        ) : null}

        <div className="finite-chat__split">
          <main className="finite-chat__main">
            <section className="finite-chat__scroll" ref={transcriptRef}>
              <div className="finite-chat__messages">
                {selectedMessages.map((message) => (
                  <MessageRow key={`${message.room_id}:${message.message_id}`} message={message} />
                ))}
                {!state ? (
                  <EmptyState title="Starting daemon" busy />
                ) : selectedMessages.length === 0 ? (
                  <EmptyState title={selectedRoom ? selectedRoom.display_name : "Finite Chat"} />
                ) : null}
              </div>
            </section>

            <form className="finite-chat__composer-wrap" onSubmit={submitComposer}>
              <div className="finite-chat__composer">
                <textarea
                  value={composer}
                  onChange={(event) => setComposer(event.target.value)}
                  placeholder={composerPlaceholder(state, selectedRoom, selectedTopic, selectedRoomHasCounterparty)}
                  disabled={!state || busy || !canSendToSelectedRoom}
                  autoFocus
                  onKeyDown={handleComposerKeyDown}
                />
                <div className="finite-chat__composer-actions">
                  <div className="finite-chat__composer-left">
                    <button type="button" className="finite-chat__tool-button" aria-label="Attach file" disabled>
                      <PaperclipIcon aria-hidden />
                    </button>
                    <button type="button" className="finite-chat__command-button" disabled>
                      <SparklesIcon aria-hidden />
                      {selectedRoom?.is_agent_chat ? "Hermes" : selectedRoomHasCounterparty ? "Room" : "No agent"}
                    </button>
                  </div>
                  <div className="finite-chat__composer-right">
                    <button
                      type="submit"
                      className="finite-chat__send-button"
                      aria-label="Send message"
                      disabled={!state || !composer.trim() || busy || !canSendToSelectedRoom}
                    >
                      <SendIcon aria-hidden />
                    </button>
                  </div>
                </div>
              </div>
            </form>
          </main>
        </div>
      </section>

      {showOnboarding ? (
        <DesktopOnboarding
          accountId={shortAccount}
          busy={identityBusy || busy}
          error={error}
          identityStatus={identityStatus}
          onImport={(secret) => importDesktopIdentity(secret)}
          onUseLocal={() => void finishOnboarding()}
        />
      ) : null}
    </div>
  );

  function handleComposerKeyDown(event: KeyboardEvent<HTMLTextAreaElement>) {
    if (event.key === "Enter" && (event.metaKey || event.ctrlKey)) {
      event.currentTarget.form?.requestSubmit();
    }
  }
}

function DesktopOnboarding({
  accountId,
  busy,
  error,
  identityStatus,
  onImport,
  onUseLocal,
}: {
  accountId: string;
  busy: boolean;
  error: string | null;
  identityStatus: DesktopIdentityStatus | null;
  onImport: (secret: string) => Promise<void>;
  onUseLocal: () => void;
}) {
  const [secret, setSecret] = useState("");

  async function submit(event: FormEvent) {
    event.preventDefault();
    await onImport(secret);
  }

  return (
    <div className="finite-chat__onboarding" role="dialog" aria-modal="true" aria-labelledby="finite-chat-onboarding-title">
      <section className="finite-chat__onboarding-panel">
        <div className="finite-chat__onboarding-brand">
          <FiniteBrand />
          <span>Desktop</span>
        </div>
        <div className="finite-chat__onboarding-copy">
          <h1 id="finite-chat-onboarding-title">Finite Chat</h1>
          <p>{identityStatus?.hasStoredAccountSecret ? "Imported account ready." : "Set up this device."}</p>
        </div>

        <button type="button" className="finite-chat__onboarding-choice" onClick={onUseLocal} disabled={busy}>
          <ShieldCheckIcon aria-hidden />
          <span>
            <strong>Use this Mac</strong>
            <small>{accountId}</small>
          </span>
        </button>

        <form className="finite-chat__onboarding-import" onSubmit={submit}>
          <label htmlFor="finite-chat-secret">Import account</label>
          <div>
            <input
              id="finite-chat-secret"
              value={secret}
              onChange={(event) => setSecret(event.target.value)}
              placeholder="nsec or 64-char secret"
              type="password"
              disabled={busy || identityStatus?.secureStorageAvailable === false}
            />
            <button
              type="submit"
              className="finite-chat__send-button"
              aria-label="Import account"
              disabled={!secret.trim() || busy || identityStatus?.secureStorageAvailable === false}
            >
              {busy ? <Loader2Icon className="finite-chat__spin" aria-hidden /> : <KeyRoundIcon aria-hidden />}
            </button>
          </div>
          {identityStatus?.secureStorageAvailable === false ? <span>Secure store unavailable</span> : null}
        </form>

        {error ? (
          <div className="finite-chat__onboarding-error">
            <strong>Daemon</strong>
            <span>{error}</span>
          </div>
        ) : null}
      </section>
    </div>
  );
}

function AgentConnectionPanel({
  busy,
  hasAgentRoom,
  inviteUrl,
  onInviteUrlChange,
  onSubmitInvite,
  onSubmitPendingInviteJoin,
  pendingInviteRoomId,
  selectedRoom,
}: {
  busy: boolean;
  hasAgentRoom: boolean;
  inviteUrl: string;
  onInviteUrlChange: (value: string) => void;
  onSubmitInvite: (event: FormEvent) => void;
  onSubmitPendingInviteJoin: () => void;
  pendingInviteRoomId: string | null;
  selectedRoom: AppRoomSummary | null;
}) {
  const copy = agentConnectionCopy(selectedRoom, hasAgentRoom, pendingInviteRoomId);
  return (
    <section className="finite-chat__agent-panel">
      <div className="finite-chat__agent-panel-icon" aria-hidden>
        {selectedRoom?.state === "WaitingForApproval" ? (
          <Loader2Icon className="finite-chat__spin" />
        ) : (
          <BotIcon />
        )}
      </div>
      <div className="finite-chat__agent-panel-copy">
        <strong>{copy.title}</strong>
        <span>{copy.body}</span>
      </div>
      {pendingInviteRoomId ? (
        <button type="button" className="finite-chat__command-button" onClick={onSubmitPendingInviteJoin} disabled={busy}>
          Join
        </button>
      ) : (
        <form className="finite-chat__agent-panel-form" onSubmit={onSubmitInvite}>
          <input
            value={inviteUrl}
            onChange={(event) => onInviteUrlChange(event.target.value)}
            placeholder="finite://join..."
            disabled={busy}
          />
          <button type="submit" className="finite-chat__send-button" aria-label="Connect Hermes" disabled={!inviteUrl.trim() || busy}>
            <LinkIcon aria-hidden />
          </button>
        </form>
      )}
    </section>
  );
}

function RoomThreadButton({
  active,
  onClick,
  room,
}: {
  active: boolean;
  onClick: () => void;
  room: AppRoomSummary;
}) {
  const working = room.state === "Joining" || room.state === "WaitingForApproval";
  return (
    <button
      type="button"
      aria-busy={working ? true : undefined}
      className={[active ? "is-active" : "", working ? "is-working" : ""].filter(Boolean).join(" ")}
      onClick={onClick}
    >
      <ThreadActivityIndicator active={working} />
      <span className="finite-chat__thread-main">
        <span className="finite-chat__thread-title">{room.display_name}</span>
        <span className="finite-chat__thread-time">{room.unread_count > 0 ? room.unread_count : room.state}</span>
      </span>
    </button>
  );
}

function agentConnectionCopy(
  selectedRoom: AppRoomSummary | null,
  hasAgentRoom: boolean,
  pendingInviteRoomId: string | null
) {
  if (pendingInviteRoomId || selectedRoom?.state === "WaitingForApproval" || selectedRoom?.state === "Joining") {
    return {
      title: "Waiting for Hermes",
      body: "The join request has been sent. Hermes needs to admit this device before messages can flow.",
    };
  }
  if (selectedRoom && !selectedRoom.is_agent_chat) {
    return {
      title: "No agent in this chat",
      body: "This room does not contain Hermes. Paste a Finite Chat invite from a runtime to connect an agent chat.",
    };
  }
  if (hasAgentRoom) {
    return {
      title: "Select an agent chat",
      body: "Hermes is connected in another room. Select that room or start a new topic there.",
    };
  }
  return {
    title: "Connect Hermes",
    body: "Paste a finite:// join invite from a local or hosted Hermes runtime. The desktop app will join that encrypted room as this device.",
  };
}

function composerPlaceholder(
  state: AppState | null,
  selectedRoom: AppRoomSummary | null,
  selectedTopic: AppTopicSummary | null,
  selectedRoomHasCounterparty: boolean
) {
  if (!state) {
    return "Starting daemon";
  }
  if (!selectedRoom) {
    return "Connect Hermes to chat";
  }
  if (selectedRoom.state === "WaitingForApproval" || selectedRoom.state === "Joining") {
    return "Waiting for Hermes to admit this device";
  }
  if (!selectedRoomHasCounterparty) {
    return "Connect Hermes before sending";
  }
  return `Message ${selectedTopic?.title ?? selectedRoom.display_name}`;
}

function ThreadActivityIndicator({ active }: { active: boolean }) {
  return (
    <span className={`finite-chat__thread-indicator ${active ? "is-thinking" : ""}`} aria-hidden>
      {active ? <span className="finite-chat__thread-pulse" /> : <MessageCircleIcon />}
    </span>
  );
}

function TopicStrip({
  onOpenRoom,
  onOpenTopic,
  selectedRoom,
  selectedTopic,
  topics,
}: {
  onOpenRoom: () => void;
  onOpenTopic: (topic: AppTopicSummary) => void;
  selectedRoom: AppRoomSummary;
  selectedTopic: AppTopicSummary | null;
  topics: AppTopicSummary[];
}) {
  return (
    <div className="finite-chat__topic-strip" aria-label="Topics">
      <button type="button" className={!selectedTopic ? "is-active" : ""} onClick={onOpenRoom}>
        Room
      </button>
      {topics.map((topic) => (
        <button
          key={`${topic.room_id}:${topic.topic_id}`}
          type="button"
          className={topic.topic_id === selectedTopic?.topic_id ? "is-active" : ""}
          onClick={() => onOpenTopic(topic)}
        >
          {topic.title}
          {topic.unread_count > 0 ? <span>{topic.unread_count}</span> : null}
        </button>
      ))}
      <small>{selectedRoom.display_name}</small>
    </div>
  );
}

function MembersPill({ members }: { members: AppRoomMemberSummary[] }) {
  const visible = members.slice(0, 3);
  return (
    <div className="finite-chat__members-pill" title={members.map((member) => member.display_name).join(", ")}>
      {visible.map((member) => (
        <span key={`${member.account_id}:${member.device_id}`} className="finite-chat__avatar" aria-hidden>
          {initials(member.display_name)}
        </span>
      ))}
    </div>
  );
}

function MessageRow({ message }: { message: ChatMessage }) {
  const content = message.display_content || message.text;
  if (message.is_mine) {
    return (
      <article className="finite-chat__message finite-chat__message--user">
        <div>
          <p>{content}</p>
          <time className="finite-chat__message-time">{message.display_timestamp}</time>
        </div>
      </article>
    );
  }

  return (
    <article className="finite-chat__message finite-chat__message--agent">
      <div className="finite-chat__assistant-text">
        <ReactMarkdown remarkPlugins={[remarkGfm]}>{content}</ReactMarkdown>
      </div>
      <time className="finite-chat__message-time">
        {message.sender_display_name} · {message.display_timestamp}
      </time>
    </article>
  );
}

function EmptyState({ busy, title }: { busy?: boolean; title: string }) {
  return (
    <div className="finite-chat__empty finite-chat__empty--solo">
      <span className="finite-chat__empty-logo" aria-hidden>
        {busy ? <Loader2Icon className="finite-chat__spin" /> : <MessageCircleIcon />}
      </span>
      <h1>
        <span className="finite-chat__empty-title">{title}</span>
        <span className="finite-chat__empty-type-cursor" aria-hidden />
      </h1>
    </div>
  );
}

function initials(value: string) {
  const trimmed = value.trim();
  if (!trimmed) {
    return "FC";
  }
  const parts = trimmed.split(/\s+/).slice(0, 2);
  return parts.map((part) => part[0]?.toUpperCase()).join("");
}

function shortId(value: string) {
  if (value.length <= 14) {
    return value;
  }
  return `${value.slice(0, 8)}...${value.slice(-4)}`;
}

function errorMessage(reason: unknown) {
  return reason instanceof Error ? reason.message : String(reason);
}
