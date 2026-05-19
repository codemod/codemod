import { useState, type MouseEvent } from "react";
import { Button, Input } from "@codemod.com/report-ui";
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
  SelectGroup,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@codemod.com/report-ui";
import type { ExecutionReport } from "@codemod.com/report-ui";
import { Check, Copy, Lock, LogIn, Upload } from "lucide-react";

type ShareLevel = "metricsOnly" | "withFiles";

interface ShareButtonProps {
  report: ExecutionReport;
}

export function ShareButton({ report: _report }: ShareButtonProps) {
  const hasMetrics = Object.keys(_report.metrics).length > 0;
  const hasStats = _report.diffs.length > 0;
  const hasBoth = hasMetrics && hasStats;
  const defaultLevel: ShareLevel = hasStats ? "withFiles" : "metricsOnly";

  const [open, setOpen] = useState(false);
  const [level, setLevel] = useState<ShareLevel>(defaultLevel);
  const [state, setState] = useState<"idle" | "publishing" | "published" | "error">("idle");
  const [errorMsg, setErrorMsg] = useState("");
  const [shareUrl, setShareUrl] = useState("");
  const [copied, setCopied] = useState(false);
  const [authState, setAuthState] = useState<"checking" | "authenticated" | "unauthenticated">(
    "checking",
  );
  const [isLoggingIn, setIsLoggingIn] = useState(false);

  const shareOptions: { label: string; value: ShareLevel }[] = [
    ...(hasBoth || hasStats
      ? [{ label: "Metrics + Stats", value: "withFiles" as ShareLevel }]
      : []),
    { label: "Metrics only", value: "metricsOnly" as ShareLevel },
  ];

  async function fetchAuthStatus() {
    setAuthState("checking");
    try {
      const resp = await fetch("/api/auth-status");
      if (!resp.ok) throw new Error("Failed to check authentication status");
      const data = await resp.json();
      setAuthState(data.authenticated ? "authenticated" : "unauthenticated");
    } catch {
      setAuthState("unauthenticated");
    }
  }

  function handleOpenDialog() {
    setOpen(true);
    setErrorMsg("");
    setCopied(false);
    if (state !== "published") {
      setState("idle");
      setShareUrl("");
    }
    void fetchAuthStatus();
  }

  async function copyShareLink(urlToCopy = shareUrl) {
    if (!urlToCopy) return;
    try {
      await navigator.clipboard.writeText(urlToCopy);
    } catch {
      const input = document.createElement("input");
      input.value = urlToCopy;
      document.body.appendChild(input);
      input.select();
      document.execCommand("copy");
      document.body.removeChild(input);
    }
    setCopied(true);
    setTimeout(() => setCopied(false), 2000);
  }

  async function handlePublishAndCopy() {
    setState("publishing");
    setErrorMsg("");
    try {
      const resp = await fetch("/api/share", {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ level }),
      });
      if (!resp.ok) {
        const data = await resp.json().catch(() => ({}));
        if (resp.status === 401) setAuthState("unauthenticated");
        throw new Error(data.error || `Upload failed: ${resp.status}`);
      }
      const data = await resp.json();
      const url = data.url || data.shareUrl || "";
      if (!url) throw new Error("Share URL was not returned");
      setShareUrl(url);
      await copyShareLink(url);
      setState("published");
    } catch (e: any) {
      setErrorMsg(e.message || "Failed to publish");
      setState("error");
    }
  }

  async function handleLogin() {
    setIsLoggingIn(true);
    setErrorMsg("");
    try {
      const resp = await fetch("/api/login", { method: "POST" });
      if (!resp.ok) {
        const data = await resp.json().catch(() => ({}));
        throw new Error(data.error || "Login failed");
      }
      setAuthState("authenticated");
    } catch (e: any) {
      setErrorMsg(e.message || "Login failed");
    } finally {
      setIsLoggingIn(false);
    }
  }

  return (
    <>
      <Button onClick={handleOpenDialog}>
        <Upload className="size-4" />
        Share
      </Button>

      <Dialog open={open} onOpenChange={setOpen}>
        <DialogContent className="max-w-xl border-border/60 bg-background p-4 shadow-2xl">
          <DialogHeader className="mb-1">
            <DialogTitle className="text-xl font-semibold text-foreground">Share</DialogTitle>
            <DialogDescription className="sr-only">
              Configure scope and publish a share link.
            </DialogDescription>
          </DialogHeader>

          <div className="rounded-xl border border-border/70 bg-card p-4">
            {authState === "unauthenticated" ? (
              <div className="rounded-lg border border-border bg-background p-6 text-center">
                <Lock className="mx-auto size-12 text-muted-foreground" />
                <p className="mt-3 text-xl font-semibold text-foreground">
                  Login required to share
                </p>
                <p className="mt-2 text-base text-muted-foreground">
                  run{" "}
                  <code className="rounded bg-muted px-2 py-1 text-foreground">codemod login</code>{" "}
                  in your terminal.
                </p>
                <Button
                  onClick={handleLogin}
                  disabled={isLoggingIn}
                  className="mt-5 h-11 min-w-36 text-sm font-semibold"
                >
                  <LogIn className="size-4" />
                  {isLoggingIn ? "Logging in..." : "Login"}
                </Button>
              </div>
            ) : authState === "checking" ? (
              <div className="rounded-lg border border-border bg-background p-6 text-center text-muted-foreground">
                Checking authentication...
              </div>
            ) : (
              <>
                <p className="mb-2 text-xs font-semibold tracking-wide text-muted-foreground">
                  SHARE SCOPE
                </p>
                <div className="mb-4">
                  <Select
                    items={shareOptions}
                    value={level}
                    onValueChange={(v: ShareLevel) => {
                      if (v === level) return;
                      setLevel(v);
                      if (state === "published") {
                        setState("idle");
                        setShareUrl("");
                        setCopied(false);
                      }
                    }}
                  >
                    <SelectTrigger className="h-11 w-full rounded-lg border-border bg-background">
                      <SelectValue />
                    </SelectTrigger>
                    <SelectContent>
                      <SelectGroup>
                        {shareOptions.map((opt) => (
                          <SelectItem key={opt.value} value={opt.value}>
                            {opt.label}
                          </SelectItem>
                        ))}
                      </SelectGroup>
                    </SelectContent>
                  </Select>
                </div>

                {state === "published" && shareUrl && (
                  <div className="mb-4">
                    <p className="mb-2 text-sm text-muted-foreground">Share link</p>
                    <div className="flex items-center gap-2">
                      <Input
                        type="text"
                        value={shareUrl}
                        readOnly
                        className="h-11 border-border bg-background font-mono text-sm"
                        onClick={(e: MouseEvent<HTMLInputElement>) => e.currentTarget.select()}
                      />
                      <Button
                        variant="outline"
                        className="size-11! p-0"
                        onClick={() => copyShareLink()}
                        aria-label="Copy share link"
                      >
                        {copied ? <Check className="size-4" /> : <Copy className="size-4" />}
                      </Button>
                    </div>
                  </div>
                )}

                <div className="mb-4 border-t border-border/60 pt-4">
                  <p className="text-sm text-muted-foreground">
                    Sharing uploads stats and results to Codemod servers. No source code is stored.
                  </p>
                </div>

                <DialogFooter>
                  <Button
                    variant="default"
                    onClick={state === "published" ? () => copyShareLink() : handlePublishAndCopy}
                    disabled={state === "publishing"}
                    className="h-11 w-full text-sm font-semibold"
                  >
                    {state === "published" || copied ? (
                      <Check className="size-4" />
                    ) : (
                      <Upload className="size-4" />
                    )}
                    {state === "publishing"
                      ? "Publishing..."
                      : state === "published"
                        ? copied
                          ? "Copied"
                          : "Copy link"
                        : "Publish & copy link"}
                  </Button>
                </DialogFooter>
              </>
            )}

            {errorMsg && <p className="mt-3 text-sm text-destructive">{errorMsg}</p>}
          </div>
        </DialogContent>
      </Dialog>
    </>
  );
}
