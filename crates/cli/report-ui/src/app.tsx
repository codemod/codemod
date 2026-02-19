import { useState, useEffect } from "react";
import { ReportViewer } from "@codemod.com/report-ui";
import type { ExecutionReport } from "@codemod.com/report-ui";
import { ShareButton } from "./components/share-button";

export function App() {
  const [report, setReport] = useState<ExecutionReport | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    async function loadReport() {
      try {
        const embedded = (window as any).__REPORT_DATA__;
        if (embedded) {
          setReport(embedded);
          return;
        }

        const resp = await fetch("/api/report");
        if (!resp.ok) throw new Error(`Failed to load report: ${resp.status}`);
        const data = await resp.json();
        setReport(data);
      } catch (e: any) {
        setError(e.message || "Failed to load report");
      }
    }
    loadReport();
  }, []);

  if (error) {
    return (
      <div className="min-h-screen flex items-center justify-center">
        <div className="text-center text-destructive">
          <h2 className="text-lg font-semibold mb-2">Failed to load report</h2>
          <p className="text-sm text-muted-foreground">{error}</p>
        </div>
      </div>
    );
  }

  if (!report) {
    return (
      <div className="min-h-screen flex items-center justify-center">
        <div className="text-muted-foreground text-sm">Loading report...</div>
      </div>
    );
  }

  return <ReportViewer data={report} actions={<ShareButton report={report} />} />;
}
