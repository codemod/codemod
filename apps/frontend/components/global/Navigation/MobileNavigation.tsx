"use client";

import Icon from "@/components/shared/Icon";
import LinkButton from "@/components/shared/LinkButton";
import type { NavigationPayload, SanityLinkType } from "@/types";
import * as DropdownMenu from "@radix-ui/react-dropdown-menu";
import { AnimatePresence, type Variants, motion } from "framer-motion";
import { AreaChart, FolderKanban, Languages, Users2 } from "lucide-react";
import { usePathname } from "next/navigation";
import { useEffect, useState } from "react";
import HeaderDropdown from "./HeaderNavigation";
import NavigationLink from "./NavigationLink";

type MobileNavigationProps = {
  items: SanityLinkType[];
  visible: boolean;
  navigationCtas?: NavigationPayload["navigationItems"];
  closeFn: () => void;
};

// Mobile dropdown component
export function MobileDropdown({
  items,
  visible,
  navigationCtas,
  closeFn,
}: MobileNavigationProps) {
  const [currentTheme, setCurrentTheme] = useState("light" as "light" | "dark");
  const isDarkMode = currentTheme === "dark";
  const pathname = usePathname();

  const animationVariants: Variants = {
    initial: {
      opacity: 0,
      y: -4,
    },
    animate: (index: number) => ({
      opacity: 1,
      y: 0,
      transition: {
        duration: 0.3,
        delay: 0.042 * index,
      },
    }),
    exit: {
      opacity: 0,
      y: -4,
    },
  };

  useEffect(() => {
    const darkMatcher = window.matchMedia("(prefers-color-scheme: dark)");
    const storedTheme = localStorage.getItem("theme") as "light" | "dark";

    const theme = storedTheme || darkMatcher.matches ? "dark" : "light";

    setCurrentTheme(theme);
  }, []);

  return (
    <div className="z-[100] flex w-screen max-w-full flex-col bg-primary-dark px-xs pb-s pt-m dark:bg-primary-light">
      <AnimatePresence>
        {visible && (
          <>
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
                  label: "Internationalization",
                  description: "Replace hard-coded strings with i18n keys",
                },
              ]}
            />
            {items?.map((item, index) => (
              <MobileDropdownItem
                key={item.href}
                animationVariants={animationVariants}
                index={index}
              >
                <NavigationLink
                  isCurrent={item.href === pathname}
                  onClick={closeFn}
                  className="w-full p-s transition-colors rounded-[8px] hover:bg-primary-light/5 dark:hover:bg-primary-dark/5"
                  href={item.href}
                >
                  {item.label}
                </NavigationLink>
              </MobileDropdownItem>
            ))}
            {/* <NavigationLink
              className="w-full p-s transition-colors rounded-[8px] hover:bg-primary-light/5 dark:hover:bg-primary-dark/5"
              hideExternalIcon
              href={`https://github.com/codemod-com/codemod`}
            >
              <span className="flex items-center gap-2">
                <TechLogo
                  className="text-black dark:text-white"
                  pathClassName="dark:fill-white"
                  name={"github"}
                />
                <span className="">{"Star us"}</span>
              </span>
            </NavigationLink> */}

            <motion.div
              className="flex gap-s pt-m"
              variants={animationVariants}
              initial="initial"
              animate="animate"
              exit="exit"
              custom={6}
            >
              {navigationCtas
                ?.map((item, index) => (
                  <LinkButton
                    key={item._key}
                    className="w-full"
                    href={item?.href}
                    glow={index === 1 && isDarkMode}
                    intent={
                      isDarkMode
                        ? "secondary"
                        : index === 1
                          ? "primary"
                          : "secondary"
                    }
                  >
                    {item?.label}
                  </LinkButton>
                ))

                .reverse()}
            </motion.div>
          </>
        )}
      </AnimatePresence>
    </div>
  );
}

export function MobileDropdownItem({
  children,
  index,
  animationVariants,
}: {
  children: React.ReactNode;
  index: number;
  animationVariants?: Record<string, any>;
}) {
  return (
    <DropdownMenu.Item asChild>
      <motion.div
        className="bg-primary-dark dark:bg-primary-light"
        variants={animationVariants}
        initial="initial"
        animate="animate"
        exit="exit"
        custom={index}
      >
        {children}
      </motion.div>
    </DropdownMenu.Item>
  );
}
