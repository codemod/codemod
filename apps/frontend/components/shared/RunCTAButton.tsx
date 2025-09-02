"use client";

import { VSCODE_PREFIX } from "@/constants";
import { openLink } from "@/utils";
import type { ReactNode } from "react";
import { type ExternalToast, toast } from "sonner";
import Button from "./Button";

type RunCTAButtonProps = {
  href: string;
  title: string;
  toastMessage?: ReactNode;
  toastOptions?: ExternalToast;
};

export default function RunCTAButton({
  href,
  title,
  toastMessage,
  toastOptions,
}: RunCTAButtonProps) {
  return (
    <Button
      iconPosition="left"
      icon={
        href.startsWith(VSCODE_PREFIX) ? (
          "noborder-vscode"
        ) : (
          <img
            src="/icons/cursor-ide.svg"
            width={20}
            height={20}
            alt="cursor-ide-svg"
            style={{ marginLeft: "-1rem" }}
          />
        )
      }
      intent="secondary"
      onClick={() => {
        if (toastMessage) {
          toast(toastMessage, toastOptions);
        }
        openLink(href);
      }}
    >
      {title}
    </Button>
  );
}
