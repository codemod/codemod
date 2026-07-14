import { SquareArrowOutUpRight } from "lucide-react";

interface RegistryButtonProps {
  url: string;
}

export function RegistryButton({ url }: RegistryButtonProps) {
  return (
    <a
      href={url}
      target="_blank"
      rel="noopener noreferrer"
      className="inline-flex items-center gap-2 text-sm px-1.5 hover:text-foreground transition-colors hover:underline text-muted-foreground"
    >
      View in Registry
      <SquareArrowOutUpRight className="size-3.5" />
    </a>
  );
}
