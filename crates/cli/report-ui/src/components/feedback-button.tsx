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
import { Check, MessageSquare, Send, Sparkles } from "lucide-react";

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

type FeedbackState = "idle" | "loading" | "submitting" | "submitted" | "error";
type FeedbackMode = "choice" | "manual";

type FeedbackStreamEvent =
  | { type: "agent"; agent: AgentOption }
  | { type: "status"; message: string }
  | { type: "output"; text: string; stream?: string }
  | { type: "done"; message: string; agent?: AgentOption }
  | { type: "error"; error: string };

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

  const supportedAgentOrder = ["claude-code", "codex", "opencode", "goose"];
  const supportedAgents =
    status?.agents
      .filter((agent) => agent.available && supportedAgentOrder.includes(agent.canonical))
      .sort(
        (a, b) =>
          supportedAgentOrder.indexOf(a.canonical) - supportedAgentOrder.indexOf(b.canonical),
      ) ?? [];
  const preferredAgentOption =
    supportedAgents.find((agent) => agent.canonical === preferredAgent) ??
    status?.selectedAgent ??
    null;
  const autoAgent = activeAgent ?? preferredAgentOption;
  const autoAgentLabel = autoAgent?.label ?? "an available agent";
  const canAutoSubmit = !status?.disabled && autoAgent !== null;
  const canManualSubmit = !status?.disabled && message.trim().length > 0;
  const activityContent =
    state === "submitted" && message
      ? message
      : [agentLog.join("\n"), liveDraft].filter(Boolean).join("\n\n") ||
        "Agent activity will appear here while auto-submit runs.";

  async function fetchStatus() {
    setState("loading");
    try {
      const resp = await fetch("/api/feedback/status");
      if (!resp.ok) throw new Error("Failed to load feedback status");
      const data = await resp.json();
      setStatus(data);
      setPreferredAgent(data.selectedAgent?.canonical ?? "");
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
    setPreferredAgent("");
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
    } catch (e: any) {
      setErrorMsg(e.message || "Failed to submit feedback");
      setState("error");
    }
  }

  async function submitAgentFeedback() {
    setState("submitting");
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
      setState("submitted");
    } catch (e: any) {
      setErrorMsg(e.message || "Failed to submit feedback");
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
      setAgentLog((current) => [...current, "Feedback submitted."]);
      return;
    }

    if (event.type === "error") {
      throw new Error(event.error || "Agent feedback failed");
    }
  }

  return (
    <>
      <Button onClick={handleOpenDialog}>
        <MessageSquare className="size-4" />
        Feedback
      </Button>

      <Dialog open={open} onOpenChange={setOpen}>
        <DialogContent className="max-h-[90vh] max-w-xl overflow-hidden border-border/60 bg-background p-4 shadow-2xl">
          <DialogHeader className="mb-1">
            <DialogTitle className="text-xl font-semibold text-foreground">Feedback</DialogTitle>
            <DialogDescription className="sr-only">
              Submit codemod performance feedback.
            </DialogDescription>
          </DialogHeader>

          <div className="max-h-[calc(90vh-7rem)] overflow-y-auto overflow-x-hidden rounded-xl border border-border/70 bg-card p-4">
            {state === "loading" ? (
              <div className="rounded-lg border border-border bg-background p-6 text-center text-muted-foreground">
                Checking available agents...
              </div>
            ) : status?.disabled ? (
              <div className="rounded-lg border border-border bg-background p-6 text-center text-muted-foreground">
                Feedback is disabled by DISABLE_ANALYTICS.
              </div>
            ) : (
              <>
                {mode === "choice" ? (
                  <div className="grid gap-3">
                    <div className="rounded-lg border border-border bg-background p-3">
                      <p className="text-sm font-semibold text-foreground">
                        Auto-submit with local agent
                      </p>
                      <p className="mt-1 text-sm text-muted-foreground">
                        Uses your local {autoAgentLabel} install to inspect the target read-only,
                        draft anonymous feedback, and submit it.
                      </p>
                      <div className="mt-3">
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
                          <SelectTrigger className="h-11 w-full rounded-lg border-border bg-background">
                            <SelectValue placeholder="No supported local agent found" />
                          </SelectTrigger>
                          <SelectContent>
                            {supportedAgents.map((agent) => (
                              <SelectItem key={agent.canonical} value={agent.canonical}>
                                {agent.label}
                              </SelectItem>
                            ))}
                          </SelectContent>
                        </Select>
                      </div>
                    </div>
                    <Button
                      variant="default"
                      onClick={submitAgentFeedback}
                      disabled={!canAutoSubmit || state === "submitting"}
                      className="h-11 w-full text-sm font-semibold"
                    >
                      {state === "submitted" ? (
                        <Check className="size-4" />
                      ) : (
                        <Sparkles className="size-4" />
                      )}
                      {state === "submitting"
                        ? "Submitting..."
                        : state === "submitted"
                          ? "Submitted"
                          : `Auto-submit with ${autoAgentLabel}`}
                    </Button>
                    <Button
                      variant="outline"
                      onClick={() => {
                        setMode("manual");
                        setState("idle");
                        setErrorMsg("");
                      }}
                      disabled={state === "submitting"}
                      className="h-11 w-full text-sm font-semibold"
                    >
                      <MessageSquare className="size-4" />
                      Manually submit feedback
                    </Button>

                    {(state === "submitting" || agentLog.length > 0 || message) && (
                      <div className="rounded-lg border border-border bg-background p-3">
                        <p className="mb-2 text-xs font-semibold tracking-wide text-muted-foreground">
                          AGENT ACTIVITY
                        </p>
                        <div className="max-h-72 min-h-32 overflow-auto whitespace-pre-wrap text-sm text-foreground">
                          {activityContent}
                        </div>
                      </div>
                    )}
                  </div>
                ) : (
                  <>
                    <textarea
                      value={message}
                      onChange={(event) => {
                        setMessage(event.currentTarget.value);
                        if (state === "submitted") setState("idle");
                      }}
                      rows={7}
                      className="mb-4 min-h-36 w-full resize-y rounded-lg border border-border bg-background px-3 py-2 text-sm text-foreground outline-none focus-visible:ring-2 focus-visible:ring-ring"
                      placeholder="What worked, what failed, or what should this codemod improve?"
                    />

                    <DialogFooter>
                      <Button
                        variant="outline"
                        onClick={() => {
                          setMode("choice");
                          setErrorMsg("");
                        }}
                        disabled={state === "submitting"}
                        className="h-11 text-sm font-semibold"
                      >
                        Back
                      </Button>
                      <Button
                        variant="default"
                        onClick={submitManualFeedback}
                        disabled={
                          !canManualSubmit || state === "submitting" || state === "submitted"
                        }
                        className="h-11 text-sm font-semibold"
                      >
                        {state === "submitted" ? (
                          <Check className="size-4" />
                        ) : (
                          <Send className="size-4" />
                        )}
                        {state === "submitting"
                          ? "Submitting..."
                          : state === "submitted"
                            ? "Submitted"
                            : "Submit feedback"}
                      </Button>
                    </DialogFooter>
                  </>
                )}
              </>
            )}

            {errorMsg && <p className="mt-3 text-sm text-destructive">{errorMsg}</p>}
          </div>
        </DialogContent>
      </Dialog>
    </>
  );
}
