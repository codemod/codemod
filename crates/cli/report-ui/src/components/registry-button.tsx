import { SquareArrowOutUpRight } from "lucide-react";

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
    <a
      href={href}
      target="_blank"
      rel="noopener noreferrer"
      className="inline-flex items-center gap-2 text-sm px-1.5 hover:text-foreground transition-colors hover:underline text-muted-foreground"
    >
      View in Registry
      <SquareArrowOutUpRight className="size-3.5" />
    </a>
  );
}
