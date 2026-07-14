import { Button } from "@codemod.com/report-ui";
import { ExternalLink } from "lucide-react";

const FALLBACK_REGISTRY_HOME = "https://app.codemod.com/registry";

interface RegistryButtonProps {
  url?: string | null;
}

function resolveRegistryHref(url?: string | null): string {
  const candidate = url?.trim();
  if (!candidate) {
    return FALLBACK_REGISTRY_HOME;
  }

  try {
    const parsed = new URL(candidate);
    if (parsed.protocol === "http:" || parsed.protocol === "https:") {
      return parsed.toString();
    }
  } catch {
    // Fall through to the safe homepage.
  }

  return FALLBACK_REGISTRY_HOME;
}

export function RegistryButton({ url }: RegistryButtonProps) {
  const href = resolveRegistryHref(url);

  return (
    <Button
      variant="outline"
      nativeButton={false}
      render={<a href={href} target="_blank" rel="noopener noreferrer" />}
    >
      <ExternalLink className="size-4" />
      Registry
    </Button>
  );
}
