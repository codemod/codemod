import { useState } from "react";
import {
  Button,
  Input,
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@codemod.com/report-ui";
import type { ExecutionReport } from "@codemod.com/report-ui";
import { Share2, Copy, Check, RotateCcw, LogIn } from "lucide-react";

type ShareLevel = "metricsOnly" | "withFiles";

interface ShareButtonProps {
  report: ExecutionReport;
}

export function ShareButton({ report }: ShareButtonProps) {
  const hasMetrics = Object.keys(report.metrics).length > 0;
  const hasStats = report.diffs.length > 0;
  const hasBoth = hasMetrics && hasStats;

  const defaultLevel: ShareLevel = hasStats ? "withFiles" : "metricsOnly";

  const [state, setState] = useState<
    "idle" | "loading" | "success" | "error" | "needs-auth" | "logging-in"
  >("idle");
  const [level, setLevel] = useState<ShareLevel>(defaultLevel);
  const [shareUrl, setShareUrl] = useState("");
  const [errorMsg, setErrorMsg] = useState("");
  const [copied, setCopied] = useState(false);

  async function handleShare() {
    setState("loading");
    try {
      const resp = await fetch("/api/share", {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ level }),
      });
      if (resp.status === 401) {
        setState("needs-auth");
        return;
      }
      if (!resp.ok) {
        const data = await resp.json().catch(() => ({}));
        throw new Error(data.error || `Upload failed: ${resp.status}`);
      }
      const data = await resp.json();
      setShareUrl(data.url || data.shareUrl || "");
      setState("success");
    } catch (e: any) {
      setErrorMsg(e.message || "Failed to share report");
      setState("error");
    }
  }

  async function handleLogin() {
    setState("logging-in");
    try {
      const resp = await fetch("/api/login", { method: "POST" });
      if (!resp.ok) {
        const data = await resp.json().catch(() => ({}));
        throw new Error(data.error || "Login failed");
      }
      // Login succeeded — automatically retry sharing
      await handleShare();
    } catch (e: any) {
      setErrorMsg(e.message || "Login failed");
      setState("error");
    }
  }

  async function handleCopy() {
    try {
      await navigator.clipboard.writeText(shareUrl);
    } catch {
      const input = document.createElement("input");
      input.value = shareUrl;
      document.body.appendChild(input);
      input.select();
      document.execCommand("copy");
      document.body.removeChild(input);
    }
    setCopied(true);
    setTimeout(() => setCopied(false), 2000);
  }

  if (state === "success" && shareUrl) {
    return (
      <div className="flex items-center gap-2">
        <Input
          type="text"
          value={shareUrl}
          readOnly
          className="w-[320px] text-green-400 font-mono text-xs"
          // @ts-expect-error -- No types
          onClick={(e) => (e.target as HTMLInputElement).select()}
        />
        <Button variant="outline" size="sm" onClick={handleCopy}>
          {copied ? <Check className="h-3.5 w-3.5 mr-1" /> : <Copy className="h-3.5 w-3.5 mr-1" />}
          {copied ? "Copied" : "Copy"}
        </Button>
      </div>
    );
  }

  if (state === "needs-auth" || state === "logging-in") {
    return (
      <div className="flex items-center gap-3">
        <span className="text-xs text-muted-foreground">
          Login required to share.{" "}
          <span className="text-muted-foreground/70">
            Or run <code className="bg-muted px-1 py-0.5 rounded text-[11px]">codemod login</code>{" "}
            in your terminal.
          </span>
        </span>
        <Button variant="outline" size="sm" onClick={handleLogin} disabled={state === "logging-in"}>
          <LogIn className="h-3.5 w-3.5 mr-1" />
          {state === "logging-in" ? "Logging in..." : "Log in"}
        </Button>
      </div>
    );
  }

  if (state === "error") {
    return (
      <div className="flex items-center gap-3">
        <span className="text-xs text-destructive">{errorMsg}</span>
        <Button variant="outline" size="sm" onClick={() => setState("idle")}>
          <RotateCcw className="h-3.5 w-3.5 mr-1" />
          Retry
        </Button>
      </div>
    );
  }

  return (
    <div className="flex flex-col gap-2">
      <div className="flex items-center gap-2">
        {hasBoth && (
          <Select value={level} onValueChange={(v: ShareLevel) => setLevel(v)}>
            <SelectTrigger className="w-48 h-9">
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              <SelectItem value="withFiles">Metrics + stats</SelectItem>
              <SelectItem value="metricsOnly">Metrics only</SelectItem>
            </SelectContent>
          </Select>
        )}
        <Button variant="outline" onClick={handleShare} disabled={state === "loading"}>
          <Share2 className="h-4 w-4 mr-2" />
          {state === "loading" ? "Uploading..." : "Share"}
        </Button>
      </div>
      <p className="text-[11px] text-muted-foreground/70">
        Sharing uploads stats and results to Codemod servers. No source code is stored.
      </p>
    </div>
  );
}
