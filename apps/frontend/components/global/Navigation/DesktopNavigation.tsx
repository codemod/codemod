"use client";

import AuthProvider from "@/app/context/AuthProvider";
import { GithubPermissions } from "@/components/GithubPermissions";
import { TokenBuilder } from "@/components/TokenBuilder";
import Button from "@/components/shared/Button";
import { TechLogo } from "@/components/shared/Icon";
import Icon from "@/components/shared/Icon";
import type { NavigationPayload, SanityLinkType } from "@/types";
import AuthButtons from "@auth/AuthButtons";
import { cx } from "cva";
import { AreaChart, FolderKanban, Languages, Users2 } from "lucide-react";
import { usePathname } from "next/navigation";
import { useCallback, useEffect, useRef, useState } from "react";
import HeaderDropdown from "./HeaderNavigation";
import NavigationLink from "./NavigationLink";

type DesktopNavigationProps = {
  items: (SanityLinkType & { isCurrent?: boolean })[];
};

// Desktop navigation items
export function DesktopNavigationItems({ items }: DesktopNavigationProps) {
  const [tXAmount, setTXAmount] = useState({ solid: 0, shadow: 0 });
  const outlineRefs = useRef<HTMLDivElement[]>([]);
  const navRef = useRef<HTMLDivElement | null>(null);
  const [activeHeadingIndex, setActiveHeadingIndex] = useState(0);
  const [width, setWidth] = useState({ solid: 0, shadow: 0 });
  const updateTXAmount = useCallback(
    (index: number, type: "solid" | "shadow") => {
      const tocRect = navRef.current?.getBoundingClientRect();
      const selectedOutlineRect =
        outlineRefs.current[index]?.getBoundingClientRect();
      setWidth({
        ...width,
        [type]: Number(selectedOutlineRect?.width),
      });
      const xDiff = Number(selectedOutlineRect?.left) - tocRect?.left!;
      setTXAmount({ ...tXAmount, [type]: xDiff });
    },
    [tXAmount, width],
  );

  useEffect(() => {
    const activeIndex = items?.findIndex((item) => item.isCurrent) ?? -1;
    setActiveHeadingIndex(activeIndex);

    updateTXAmount(activeIndex, "solid");

    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [items]);

  const handleClick = (
    item: SanityLinkType & { isCurrent?: boolean },
    index: number,
  ) => {
    setActiveHeadingIndex(index);
    updateTXAmount(index, "solid");
  };

  return (
    <div
      className={
        "group/nav relative hidden gap-l lg:flex lg:flex-1 lg:items-center lg:justify-center"
      }
      ref={navRef}
    >
      {activeHeadingIndex !== -1 && (
        <div
          style={{
            transform: `translateX(${tXAmount.solid}px)`,
            width: `${width.solid}px`,
          }}
          className="dot absolute -bottom-2 left-0 h-px bg-black transition-all duration-150 ease-in-out group-hover/nav:scale-100 group-hover/nav:opacity-100 dark:bg-white"
        />
      )}
      {
        <div
          style={{
            transform: `translateX(${tXAmount.shadow}px)`,
            width: `${width.shadow}px`,
          }}
          className="shadow-dot absolute -bottom-2 left-0 h-px bg-black/10 opacity-0 transition-all duration-150 ease-in-out group-hover/nav:scale-100 group-hover/nav:opacity-100 dark:bg-white/10"
        />
      }
      <HeaderDropdown
        label="Platform"
        items={[
          {
            category: "Plan",
            href: "https://app.codemod.com/insights",
            icon: (
              <AreaChart className="size-5 transition-colors group-hover:text-black" />
            ),
            label: "Insights",
          },
          {
            category: "Automate",
            href: "https://app.codemod.com/studio",
            icon: (
              <Icon
                name="codemod-studio"
                className="size-5 transition-colors group-hover:text-black"
              />
            ),
            label: "Studio",
          },

          {
            category: "Orchestrate",
            href: "https://app.codemod.com/projects",
            icon: (
              <FolderKanban className="size-5 transition-colors group-hover:text-black" />
            ),
            label: "Campaigns",
          },
        ]}
      />
      <HeaderDropdown
        label="Solution"
        items={[
          {
            href: "https://app.codemod.com/pages/insights",
            icon: (
              <Users2 className="size-5 transition-colors group-hover:text-black" />
            ),
            label: "Engineer leaders",
            description: "Business-aligned code-driven dashboards",
          },
          {
            href: "/i18n",
            icon: (
              <Languages className="size-5 transition-colors group-hover:text-black" />
            ),
            label: "Localization",
            description: "Replace hard-coded strings with i18n keys",
          },
        ]}
      />

      {items?.map((item, index) => (
        <NavigationLink className={cx()} key={item.href} href={item.href}>
          <span
            onClick={() => handleClick(item, index)}
            onMouseEnter={() => updateTXAmount(index, "shadow")}
            ref={(el) => {
              outlineRefs.current[index] = el as HTMLDivElement;
            }}
          >
            {item.label}
          </span>
        </NavigationLink>
      ))}
      <NavigationLink
        hideExternalIcon
        href={`https://github.com/codemod-com/codemod`}
      >
        <span
          className="flex items-center gap-2"
          onMouseEnter={() => updateTXAmount(items.length, "shadow")}
          ref={(el) => {
            outlineRefs.current[items.length] = el as HTMLDivElement;
          }}
        >
          <TechLogo
            className="text-black dark:text-white"
            pathClassName="dark:fill-white"
            name={"github"}
          />
          <span className="">{"Star us"}</span>
        </span>
      </NavigationLink>
    </div>
  );
}

// Desktop navigation right
export function DesktopNavigationRight(props: {
  items: NavigationPayload["navigationCtas"];
}) {
  const pathname = usePathname();
  const [shouldRenderAuth, setShouldRenderAuth] = useState(false);

  useEffect(() => {
    setShouldRenderAuth(true);
  }, []);

  return (
    <div className="hidden gap-3 lg:flex lg:items-center lg:justify-center">
      {props.items?.map((item, index) => (
        <NavigationLink hideExternalIcon key={item._key} href={item?.href}>
          <Button
            glow={index === 1}
            intent={index === 1 ? "secondary" : "inline"}
          >
            {item?.label}
          </Button>
        </NavigationLink>
      ))}

      {shouldRenderAuth && (
        <AuthProvider>
          <div className="hidden">
            <AuthButtons variant="www" redirectUrl={pathname} />
          </div>

          <TokenBuilder />
          <GithubPermissions />
        </AuthProvider>
      )}
    </div>
  );
}
