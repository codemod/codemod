import Icon, {
  codemodBrandString,
  codemodLogoString,
} from "@/components/shared/Icon";
import Logo from "@/components/shared/Logo";
import { CODEMOD_STUDIO_URL } from "@/constants";
import * as DropdownMenu from "@radix-ui/react-dropdown-menu";
import Link from "next/link";
import { usePathname, useRouter } from "next/navigation";
import { useEffect, useState } from "react";
import { toast } from "sonner";

export default function LogoWithContextMenu() {
  const router = useRouter();
  const pathname = usePathname();
  const [open, setOpen] = useState(false);
  let pressTimer: NodeJS.Timeout;

  function handleTouchStart(event: React.TouchEvent) {
    event.preventDefault();
    // Start a timer on touch start
    pressTimer = setTimeout(() => setOpen(true), 500);
  }

  function handleTouchEnd(event: React.TouchEvent) {
    event.preventDefault();
    // If touch ends and timer is still active, clear it
    if (!open) {
      navigateHome();
    }
    if (pressTimer) clearTimeout(pressTimer);
  }

  const copyToClipboard = (svg: string) => {
    if (navigator?.clipboard?.writeText) {
      navigator.clipboard.writeText(svg);
    }
  };

  function handleRightClick(event: React.MouseEvent) {
    event.preventDefault();
    if (event.button === 2) {
      setOpen(true);
    }
  }

  function navigateHome() {
    setOpen(false);
    router.push("/");
  }

  useEffect(() => {
    router.prefetch("/");
    setOpen(false);
  }, [pathname, router]);

  return (
    <DropdownMenu.Root open={open}>
      <DropdownMenu.Trigger
        onContextMenu={handleRightClick}
        onClick={navigateHome}
        onTouchStart={handleTouchStart}
        onTouchEnd={handleTouchEnd}
        className="select-none py-px"
        name="Logo Button"
        aria-label="Right click for context menu"
      >
        <Logo />
      </DropdownMenu.Trigger>

      <DropdownMenu.Portal>
        <DropdownMenu.Content
          align="start"
          side="bottom"
          sideOffset={16}
          onCloseAutoFocus={(event) => event.preventDefault()}
          onEscapeKeyDown={() => setOpen(false)}
          onPointerDownOutside={() => setOpen(false)}
          className="z-[99] min-w-[250px] animate-slideDownAndFade select-none rounded-[8px] border-[1px] border-border-light bg-primary-dark p-s shadow-sm dark:border-border-dark dark:bg-primary-light dark:shadow-none"
        >
          <div className="body-s-medium font-medium text-secondary-light dark:text-secondary-dark">
            Brand
          </div>
          <DropdownMenu.Group className="flex flex-col border-b-[1px] border-b-border-light py-s dark:border-b-border-dark">
            <DropdownMenu.Item
              className="body-s-medium flex cursor-pointer items-center gap-xs rounded-[8px] p-xs font-medium text-primary-light focus:outline-none data-[highlighted]:bg-emphasis-light dark:text-primary-dark dark:data-[highlighted]:bg-emphasis-dark"
              onSelect={() => {
                copyToClipboard(codemodBrandString);
                setOpen(false);
                toast("Logo copied to clipboard!", {
                  icon: (
                    <Icon
                      name="check"
                      className="text-primary-light dark:text-accent"
                    />
                  ),
                  className: "flex items-center gap-xs",
                });
              }}
            >
              <Icon name="codemod-brand" className="h-5 w-5" />
              <span>Copy logo as a SVG</span>
            </DropdownMenu.Item>
            <DropdownMenu.Item
              className="body-s-medium flex cursor-pointer items-center gap-xs rounded-[8px] p-xs font-medium text-primary-light focus:outline-none data-[highlighted]:bg-emphasis-light dark:text-primary-dark dark:data-[highlighted]:bg-emphasis-dark"
              onSelect={() => {
                copyToClipboard(codemodLogoString);
                setOpen(false);
                toast("Wordmark copied to clipboard!", {
                  icon: (
                    <Icon
                      name="check"
                      className="text-primary-light dark:text-accent"
                    />
                  ),
                  className: "flex items-center gap-xs",
                });
              }}
            >
              <Icon name="type" className="h-5 w-5" />
              <span>Copy wordmark as a SVG</span>
            </DropdownMenu.Item>
            {/* <DropdownMenu.Item asChild>
              <a
                href="https://brand.codemod.com/"
                target="_blank"
                rel="noopener noreferrer"
                className="body-s-medium flex items-center gap-xs rounded-[8px] p-xs font-medium text-primary-light focus:outline-none data-[highlighted]:bg-emphasis-light dark:text-primary-dark dark:data-[highlighted]:bg-emphasis-dark"
              >
                <Icon name="drafting" className="h-5 w-5" />
                <span>Brand guidelines</span>
              </a>
            </DropdownMenu.Item> */}
            <DropdownMenu.Item asChild>
              <Link
                href="/"
                prefetch
                className="body-s-medium flex items-center gap-xs rounded-[8px] p-xs font-medium text-primary-light focus:outline-none data-[highlighted]:bg-emphasis-light dark:text-primary-dark dark:data-[highlighted]:bg-emphasis-dark"
              >
                <Icon name="home" className="h-5 w-5" />
                <span>Go to homepage</span>
              </Link>
            </DropdownMenu.Item>
          </DropdownMenu.Group>

          <div className="body-s-medium pt-s font-medium text-secondary-light dark:text-secondary-dark">
            Platform
          </div>
          <DropdownMenu.Group className="pt-s">
            <DropdownMenu.Item asChild>
              <a
                href={CODEMOD_STUDIO_URL}
                target="_blank"
                rel="noopener noreferrer"
                className="body-s-medium flex items-center gap-xs rounded-[8px] p-xs font-medium text-primary-light focus:outline-none data-[highlighted]:bg-emphasis-light dark:text-primary-dark dark:data-[highlighted]:bg-emphasis-dark"
              >
                <Icon name="codemod-studio" className="h-5 w-5" />
                <span>Codemod Studio</span>
              </a>
            </DropdownMenu.Item>
            <DropdownMenu.Item asChild>
              <a
                href="https://codemod.com/registry/"
                target="_blank"
                rel="noopener noreferrer"
                className="body-s-medium flex items-center gap-xs rounded-[8px] p-xs font-medium text-primary-light focus:outline-none data-[highlighted]:bg-emphasis-light dark:text-primary-dark dark:data-[highlighted]:bg-emphasis-dark"
              >
                <Icon name="file" className="h-5 w-5" />
                <span>Automation Registry</span>
              </a>
            </DropdownMenu.Item>
            <DropdownMenu.Item asChild>
              <Link
                href="/cli"
                prefetch
                className="body-s-medium flex items-center gap-xs rounded-[8px] p-xs font-medium text-primary-light focus:outline-none data-[highlighted]:bg-emphasis-light dark:text-primary-dark dark:data-[highlighted]:bg-emphasis-dark"
              >
                <Icon name="terminal" className="h-5 w-5" />
                <span>CLI</span>
              </Link>
            </DropdownMenu.Item>
          </DropdownMenu.Group>
        </DropdownMenu.Content>
      </DropdownMenu.Portal>
    </DropdownMenu.Root>
  );
}
