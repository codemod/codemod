import { Button } from "@codemod.com/report-ui";
import { Globe } from "lucide-react";

interface RegistryButtonProps {
  url: string;
}

export function RegistryButton({ url }: RegistryButtonProps) {
  function openRegistry() {
    window.open(url, "_blank", "noopener,noreferrer");
  }

  return (
    <Button variant="outline" onClick={openRegistry}>
      <Globe className="size-4" />
      Registry
    </Button>
  );
}
