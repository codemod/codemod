import { cn } from "@/utils";
import type { ButtonHTMLAttributes, ReactNode } from "react";

type Props = ButtonHTMLAttributes<HTMLButtonElement> & {
  children?: ReactNode;
  tooltip?: string;
  isActive?: boolean;
};

const IconButton = ({ children, tooltip, isActive, ...restProps }: Props) => (
  <>
    <button
      className={cn(
        "mr-1 inline-flex items-center rounded-lg px-1.5 text-center text-sm text-white focus:outline-none",
        "hover:bg-blue-900 dark:hover:bg-blue-800",
        !isActive && "bg-border-light dark:bg-border-dark",
        isActive && "bg-blue-900 dark:bg-blue-800",
      )}
      data-tooltip-content={tooltip ?? ""}
      data-tooltip-id="button-tooltip"
      data-tooltip-target="tooltip-default"
      {...restProps}
    >
      {children}
    </button>
  </>
);

export default IconButton;
