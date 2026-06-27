import { Button } from "@codemod.com/report-ui";
import { ExternalLink } from "lucide-react";

interface RegistryButtonProps {
  url: string;
}

export function RegistryButton({ url }: RegistryButtonProps) {
  return (
    <Button
      variant="outline"
      nativeButton={false}
      render={<a href={url} target="_blank" rel="noopener noreferrer" />}
    >
      <ExternalLink className="size-4" />
      Registry
    </Button>
  );
}
