import { useState } from "react";
import { Button } from "@codemod.com/report-ui";
import {
  Dialog,
  DialogContent,
  DialogHeader,
  DialogTitle,
  DialogDescription,
  DialogFooter,
} from "@codemod.com/report-ui";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@codemod.com/report-ui";
import { CircleCheck, EyeOff, Loader2, MessageSquare, Pencil, Send } from "lucide-react";
import { AgentIcon } from "./agent-icons";

interface AgentOption {
  canonical: string;
  label: string;
  available: boolean;
}

interface FeedbackStatus {
  disabled: boolean;
  agents: AgentOption[];
  selectedAgent: AgentOption | null;
}

type FeedbackState = "idle" | "loading" | "drafting" | "submitting" | "submitted" | "error";
type FeedbackMode = "choice" | "manual";

type FeedbackStreamEvent =
  | { type: "agent"; agent: AgentOption }
  | { type: "status"; message: string }
  | { type: "output"; text: string; stream?: string }
  | { type: "done"; message: string; agent?: AgentOption }
  | { type: "error"; error: string };

const ACTIVITY_PLACEHOLDER = "Agent activity will appear here while the draft is prepared.";
const MAX_FEEDBACK_MESSAGE_LEN = 2500;
const SUPPORTED_AGENT_ORDER = ["claude-code", "codex", "opencode", "goose"];

function getSupportedAgents(agents: AgentOption[] | undefined): AgentOption[] {
  return (
    agents
      ?.filter((agent) => agent.available && SUPPORTED_AGENT_ORDER.includes(agent.canonical))
      .sort(
        (a, b) =>
          SUPPORTED_AGENT_ORDER.indexOf(a.canonical) - SUPPORTED_AGENT_ORDER.indexOf(b.canonical),
      ) ?? []
  );
}

function resolvePreferredAgentCanonical(current: string, status: FeedbackStatus): string {
  const supportedAgents = getSupportedAgents(status.agents);
  const supportedCanonicals = new Set(supportedAgents.map((agent) => agent.canonical));

  if (current && supportedCanonicals.has(current)) {
    return current;
  }
  if (status.selectedAgent?.canonical && supportedCanonicals.has(status.selectedAgent.canonical)) {
    return status.selectedAgent.canonical;
  }
  return supportedAgents[0]?.canonical ?? "";
}

function AgentOptionDisplay({
  agent,
  showMeta = true,
}: {
  agent: AgentOption;
  showMeta?: boolean;
}) {
  return (
    <>
      <div className="flex size-8 shrink-0 items-center justify-center rounded-md bg-muted/50">
        <AgentIcon canonical={agent.canonical} className="size-5" />
      </div>
      <div className="flex min-w-0 flex-col text-left">
        <span className="truncate text-sm leading-tight font-medium text-foreground">
          {agent.label}
        </span>
        {showMeta && (
          <span className="text-xs leading-tight text-muted-foreground">local · read-only</span>
        )}
      </div>
    </>
  );
}

export function FeedbackButton() {
  const [open, setOpen] = useState(false);
  const [status, setStatus] = useState<FeedbackStatus | null>(null);
  const [message, setMessage] = useState("");
  const [preferredAgent, setPreferredAgent] = useState("");
  const [mode, setMode] = useState<FeedbackMode>("choice");
  const [state, setState] = useState<FeedbackState>("idle");
  const [errorMsg, setErrorMsg] = useState("");
  const [agentLog, setAgentLog] = useState<string[]>([]);
  const [activeAgent, setActiveAgent] = useState<AgentOption | null>(null);
  const [liveDraft, setLiveDraft] = useState("");

  const supportedAgents = getSupportedAgents(status?.agents);
  const selectedAgentOption =
    supportedAgents.find((agent) => agent.canonical === preferredAgent) ??
    supportedAgents.find((agent) => agent.canonical === status?.selectedAgent?.canonical) ??
    supportedAgents[0] ??
    null;
  const autoAgent =
    state === "drafting" || state === "submitting" || state === "submitted"
      ? (activeAgent ?? selectedAgentOption)
      : selectedAgentOption;
  const autoAgentLabel = autoAgent?.label ?? "an available agent";
  const canAutoSubmit = !status?.disabled && selectedAgentOption !== null;
  const canManualSubmit = !status?.disabled && message.trim().length > 0;
  const activityContent =
    state === "submitted" && message
      ? message
      : [agentLog.join("\n"), liveDraft].filter(Boolean).join("\n\n") || ACTIVITY_PLACEHOLDER;
  const hasAgentDraft = agentLog.length > 0 || liveDraft.length > 0;
  const showDraftCard = state === "drafting" || (state === "error" && hasAgentDraft);

  async function fetchStatus() {
    setState("loading");
    try {
      const resp = await fetch("/api/feedback/status");
      if (!resp.ok) throw new Error("Failed to load feedback status");
      const data = await resp.json();
      setStatus(data);
      setPreferredAgent((current) => resolvePreferredAgentCanonical(current, data));
      setState("idle");
    } catch (e: any) {
      setErrorMsg(e.message || "Failed to load feedback status");
      setState("error");
    }
  }

  function handleOpenDialog() {
    setOpen(true);
    setErrorMsg("");
    setState("idle");
    setMode("choice");
    setMessage("");
    setAgentLog([]);
    setActiveAgent(null);
    setLiveDraft("");
    void fetchStatus();
  }

  async function submitManualFeedback() {
    setState("submitting");
    setErrorMsg("");
    setAgentLog([]);
    setLiveDraft("");
    try {
      const resp = await fetch("/api/feedback", {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ message }),
      });
      if (!resp.ok) {
        const data = await resp.json().catch(() => ({}));
        throw new Error(data.error || `Feedback failed: ${resp.status}`);
      }
      setState("submitted");
      setMode("choice");
    } catch (e: any) {
      setErrorMsg(e.message || "Failed to submit feedback");
      setState("error");
    }
  }

  async function draftWithAgent() {
    setState("drafting");
    setErrorMsg("");
    setMessage("");
    setLiveDraft("");
    setAgentLog([`Starting ${autoAgentLabel}...`]);
    try {
      const resp = await fetch("/api/feedback/agent/stream", {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ agent: preferredAgent || undefined }),
      });
      if (!resp.ok) {
        const data = await resp.json().catch(() => ({}));
        throw new Error(data.error || `Feedback failed: ${resp.status}`);
      }
      if (!resp.body) throw new Error("Feedback stream was not returned");

      await readFeedbackStream(resp.body);
    } catch (e: any) {
      setErrorMsg(e.message || "Failed to draft feedback");
      setState("error");
    }
  }

  async function readFeedbackStream(body: ReadableStream<Uint8Array>) {
    const reader = body.getReader();
    const decoder = new TextDecoder();
    let buffer = "";

    while (true) {
      const { value, done } = await reader.read();
      if (done) break;
      buffer += decoder.decode(value, { stream: true });
      const lines = buffer.split("\n");
      buffer = lines.pop() ?? "";
      for (const line of lines) {
        handleFeedbackStreamLine(line);
      }
    }

    const tail = buffer.trim();
    if (tail) handleFeedbackStreamLine(tail);
  }

  function handleFeedbackStreamLine(line: string) {
    if (!line.trim()) return;
    let event: FeedbackStreamEvent;
    try {
      event = JSON.parse(line) as FeedbackStreamEvent;
    } catch {
      setAgentLog((current) => [...current, `Unparsed agent output: ${line.slice(0, 240)}`]);
      return;
    }

    if (event.type === "agent") {
      setActiveAgent(event.agent);
      setAgentLog((current) => [...current, `Using ${event.agent.label} to draft feedback.`]);
      return;
    }

    if (event.type === "status") {
      setAgentLog((current) => [...current, event.message]);
      return;
    }

    if (event.type === "output") {
      setLiveDraft((current) => `${current}${event.text}`);
      return;
    }

    if (event.type === "done") {
      if (event.agent) setActiveAgent(event.agent);
      setMessage(event.message || "");
      setMode("manual");
      setState("idle");
      setAgentLog([]);
      setLiveDraft("");
      return;
    }

    if (event.type === "error") {
      throw new Error(event.error || "Agent feedback failed");
    }
  }

  function startManualFeedback() {
    setMode("manual");
    setMessage("");
    setState("idle");
    setErrorMsg("");
    setAgentLog([]);
    setLiveDraft("");
  }

  return (
    <>
      <Button onClick={handleOpenDialog}>
        <MessageSquare className="size-4" />
        Feedback
      </Button>

      <Dialog open={open} onOpenChange={setOpen}>
        <DialogContent className="max-w-md gap-4 p-5">
          <DialogHeader>
            <DialogTitle>
              {mode === "manual"
                ? message.trim()
                  ? "Review and submit feedback"
                  : "Write your own feedback"
                : "How did this codemod perform?"}
            </DialogTitle>
            <DialogDescription className="sr-only">
              Submit codemod performance feedback.
            </DialogDescription>
          </DialogHeader>

          {state === "loading" ? (
            <div className="flex items-center justify-center gap-2 py-8 text-sm text-muted-foreground">
              <Loader2 className="size-4 animate-spin" />
              Checking available agents...
            </div>
          ) : status?.disabled ? (
            <div className="py-8 text-center text-sm text-muted-foreground">
              Feedback is disabled by DISABLE_ANALYTICS.
            </div>
          ) : mode === "choice" ? (
            state === "submitted" ? (
              <div className="space-y-3">
                <div className="flex flex-col mt-10 pb-5 items-center gap-2 rounded-lg border border-success/15 bg-success-subtle px-3 py-2.5">
                  <div className="flex -mt-9 items-center justify-center p-3 rounded-full border border-success/15 bg-success-muted gap-2">
                    <CircleCheck className="size-8 shrink-0 text-success-text" />
                  </div>
                  <span className="text-base font-medium text-success-text">Feedback sent</span>
                </div>

                <div className="space-y-2 rounded-lg border border-border/60 bg-card p-3">
                  <p className="text-[10.5px] font-medium tracking-widest text-muted-foreground/50 uppercase">
                    What was sent
                  </p>
                  <p className="text-sm leading-relaxed text-foreground/50">{message}</p>
                  <div className="flex w-fit items-center gap-1.5 rounded-full border border-success/15 bg-success-subtle px-2.5 py-0.5">
                    <EyeOff className="size-3 text-success-text" />
                    <span className="text-[11px] font-medium text-success-text">
                      {activeAgent
                        ? `Sent anonymously via ${activeAgent.label}`
                        : "Sent anonymously"}
                    </span>
                  </div>
                </div>

                <Button variant="outline" size="xl" onClick={() => setOpen(false)}>
                  Done
                </Button>
              </div>
            ) : (
              <div className="space-y-4">
                {errorMsg && (
                  <p className="rounded-md border border-destructive/20 bg-destructive/10 px-3 py-2 text-xs text-destructive">
                    {errorMsg}
                  </p>
                )}

                <p className="text-sm leading-relaxed text-muted-foreground">
                  Have your agent review the changes and draft anonymized feedback to help improve
                  this codemod, or submit feedback manually.
                </p>

                <Select
                  items={supportedAgents.map((agent) => ({
                    label: agent.label,
                    value: agent.canonical,
                  }))}
                  value={preferredAgent}
                  onValueChange={(value: string) => {
                    setPreferredAgent(value);
                    setActiveAgent(null);
                  }}
                >
                  <SelectTrigger className="h-12! w-full gap-3 rounded-lg border-border/60 bg-card px-3 py-2.5 shadow-none">
                    <SelectValue
                      className="flex items-center gap-3"
                      placeholder="No supported local agent found"
                    >
                      {(value: string) => {
                        const agent =
                          supportedAgents.find((option) => option.canonical === value) ??
                          selectedAgentOption;
                        if (!agent) {
                          return (
                            <span className="text-sm text-muted-foreground">
                              No supported local agent found
                            </span>
                          );
                        }
                        return <AgentOptionDisplay agent={agent} />;
                      }}
                    </SelectValue>
                  </SelectTrigger>
                  <SelectContent className="min-w-(--anchor-width)">
                    {supportedAgents.map((agent) => (
                      <SelectItem
                        key={agent.canonical}
                        value={agent.canonical}
                        className="py-2.5 pl-3"
                      >
                        <AgentOptionDisplay agent={agent} />
                      </SelectItem>
                    ))}
                  </SelectContent>
                </Select>

                {showDraftCard && (
                  <div
                    className={`space-y-2 rounded-lg border border-border/60 bg-card p-3${state === "drafting" ? " animate-pulse" : ""}`}
                  >
                    <p className="text-[10.5px] font-medium tracking-widest text-muted-foreground/60 uppercase">
                      Draft feedback
                    </p>
                    <p className="text-sm leading-relaxed text-foreground/70">{activityContent}</p>
                    <div className="flex w-fit items-center gap-1.5 rounded-full border border-success/20 bg-success-muted px-2.5 py-0.5">
                      <EyeOff className="size-3 text-success-text" />
                      <span className="text-[11px] font-medium text-success-text">
                        Anonymized draft
                      </span>
                    </div>
                  </div>
                )}

                <div className="flex gap-2">
                  <Button
                    variant="default"
                    size="xl"
                    onClick={draftWithAgent}
                    disabled={!canAutoSubmit || state === "drafting"}
                    className="flex-1"
                  >
                    {state === "drafting" && <Loader2 className="size-3.5 animate-spin" />}
                    {state === "drafting" ? "Drafting..." : "Submit AI-Generated Feedback"}
                  </Button>
                  <Button
                    variant="outline"
                    size="xl"
                    onClick={startManualFeedback}
                    disabled={state === "drafting"}
                  >
                    <Pencil className="size-3.5" />
                    Write Manual Feedback
                  </Button>
                </div>
              </div>
            )
          ) : (
            <div className="space-y-4">
              {errorMsg && (
                <p className="rounded-md border border-destructive/20 bg-destructive/10 px-3 py-2 text-xs text-destructive">
                  {errorMsg}
                </p>
              )}

              {activeAgent && message.trim() ? (
                <div className="flex w-fit items-center gap-1.5 rounded-full border border-success/20 bg-success-muted px-2.5 py-0.5">
                  <EyeOff className="size-3 text-success-text" />
                  <span className="text-[11px] font-medium text-success-text">
                    Anonymized draft from {activeAgent.label} — edit before submitting
                  </span>
                </div>
              ) : null}

              <textarea
                value={message}
                onChange={(event) => {
                  setMessage(event.currentTarget.value);
                  if (state === "submitted") setState("idle");
                }}
                maxLength={MAX_FEEDBACK_MESSAGE_LEN}
                rows={6}
                className="w-full resize-none rounded-lg border border-border/60 bg-card px-3 py-2.5 text-sm text-foreground outline-none placeholder:text-muted-foreground/40 focus-visible:ring-1 focus-visible:ring-foreground"
                placeholder="What worked, what failed, or what should this codemod improve?"
              />
              <p className="-mt-2 text-right text-[11px] text-muted-foreground/40">
                {message.length}/{MAX_FEEDBACK_MESSAGE_LEN}
              </p>

              <DialogFooter>
                <Button
                  variant="outline"
                  onClick={() => {
                    setMode("choice");
                    setErrorMsg("");
                    setMessage("");
                    setActiveAgent(null);
                    setAgentLog([]);
                    setLiveDraft("");
                  }}
                  size="xl"
                  disabled={state === "submitting"}
                >
                  Back
                </Button>
                <Button
                  variant="default"
                  onClick={submitManualFeedback}
                  disabled={!canManualSubmit || state === "submitting" || state === "submitted"}
                  size="xl"
                >
                  {state === "submitting" ? (
                    <Loader2 className="size-3.5 animate-spin" />
                  ) : (
                    <Send className="size-3.5" />
                  )}
                  {state === "submitting" ? "Sending..." : "Submit feedback"}
                </Button>
              </DialogFooter>
            </div>
          )}
        </DialogContent>
      </Dialog>
    </>
  );
}
