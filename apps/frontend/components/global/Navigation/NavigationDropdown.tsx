import useDebounce from "@/hooks/useDebounce";
import * as DropdownMenu from "@radix-ui/react-dropdown-menu";
import { AnimatePresence, motion } from "framer-motion";
import type React from "react";
import { useState } from "react";

interface NavigationDropdownProps {
  trigger: (open: boolean) => React.ReactNode;
  children: React.ReactNode;
  align?: "start" | "center" | "end" | undefined;
  side?: "bottom" | "top" | "right" | "left" | undefined;
  sideOffset?: number;
}

export const NavigationDropdown: React.FC<NavigationDropdownProps> = ({
  trigger,
  children,
  align = "center",
  side = "bottom",
  sideOffset = 16,
}) => {
  const [open, setOpen] = useState(false);
  const debouncedOpen = useDebounce<boolean>(open, 150);

  const isMobile = typeof window !== "undefined" && window.innerWidth < 1024;

  // Mobile layout
  if (isMobile) {
    return (
      <>
        <div
          className="bg-primary-dark dark:bg-primary-light"
          onClick={() => setOpen(!open)}
        >
          {trigger(open)}
        </div>

        <AnimatePresence initial={false}>
          {open && (
            <motion.div
              key="dropdown"
              initial={{ height: 0, opacity: 0 }}
              animate={{ height: "auto", opacity: 1 }}
              exit={{ height: 0, opacity: 0 }}
              transition={{ duration: 0.3, ease: "easeInOut" }}
              className="overflow-hidden px-s"
            >
              {children}
            </motion.div>
          )}
        </AnimatePresence>
      </>
    );
  }

  // Desktop layout
  return (
    <DropdownMenu.Root
      open={debouncedOpen}
      modal={false}
      onOpenChange={setOpen}
    >
      <DropdownMenu.Trigger
        asChild
        onClick={(event) => {
          event.preventDefault();
          setOpen(true);
        }}
        onMouseEnter={(event) => {
          event.preventDefault();
          setOpen(true);
        }}
        onMouseLeave={(event) => {
          event.preventDefault();
          setOpen(false);
        }}
      >
        {trigger(open)}
      </DropdownMenu.Trigger>

      <DropdownMenu.Portal>
        <DropdownMenu.Content
          align={align}
          side={side}
          sideOffset={sideOffset}
          onMouseEnter={(event) => {
            event.preventDefault();
            setOpen(true);
          }}
          onMouseLeave={(event) => {
            event.preventDefault();
            setOpen(false);
          }}
          onCloseAutoFocus={(event) => event.preventDefault()}
          onEscapeKeyDown={() => setOpen(false)}
          onPointerDownOutside={() => setOpen(false)}
          className="z-[99] min-w-[250px] animate-slideDownAndFade select-none rounded-[8px] border-[1px] border-border-light bg-primary-dark/80 backdrop-blur-lg py-2 px-4 shadow-sm dark:border-border-dark dark:bg-primary-light/90 dark:shadow-none"
        >
          {children}
        </DropdownMenu.Content>
      </DropdownMenu.Portal>
    </DropdownMenu.Root>
  );
};
