import React from "react";
import clsx from "clsx";

interface SeparatorTicksProps {
  edge?: "top" | "bottom";
  className?: string;
  centerDot?: boolean;
  variant?: "half" | "third";
  simple?: boolean;
}

const edgeMap = {
  top: {
    padding: "pt-0",
    verticalTicks: [
      <div
        key="vl"
        className="absolute bottom-0 left-0 h-48 w-px bg-gradient-to-b from-transparent to-border-light dark:to-border-dark"
      />,
      <div
        key="vr"
        className="absolute bottom-0 right-0 h-48 w-px bg-gradient-to-b from-transparent to-border-light dark:to-border-dark"
      />,
    ],
    horizontal: "bottom-0",
    side: "bottom-0",
    dot: "-bottom-1",
  },
  bottom: {
    padding: "pb-24",
    verticalTicks: [
      <div
        key="vl"
        className="absolute left-0 top-0 h-24 w-px bg-gradient-to-t from-transparent to-border-light dark:to-border-dark"
      />,
      <div
        key="vr"
        className="absolute right-0 top-0 h-24 w-px bg-gradient-to-t from-transparent to-border-light dark:to-border-dark"
      />,
    ],
    horizontal: "top-0",
    side: "top-0",
    dot: "-top-1",
  },
};

export function SeparatorY({
  edge = "top",
  className,
  centerDot = true,
  variant = "half",
  simple = false,
}: SeparatorTicksProps) {
  const map = edgeMap[edge];
  const isThirdVariant = variant === "third";
  const showCenterDot = centerDot && !isThirdVariant;

  return (
    <div
      className={clsx(
        "pointer-events-none relative mx-auto h-px w-full",
        map.padding,
        className,
      )}
      aria-hidden="true"
    >
      {/* Horizontal lines */}
      {isThirdVariant ? (
        <>
          <div
            className={clsx(
              "absolute left-0 h-px w-[calc(33.33%-8px)] bg-border-light dark:bg-border-dark",
              map.horizontal,
            )}
          />
          <div
            className={clsx(
              "absolute left-[calc(33.33%+18px)] h-px w-[calc(33.33%-36px)] bg-border-light dark:bg-border-dark",
              map.horizontal,
            )}
          />
          <div
            className={clsx(
              "absolute right-0 h-px w-[calc(33.33%-8px)] bg-border-light dark:bg-border-dark",
              map.horizontal,
            )}
          />
        </>
      ) : (
        <>
          <div
            className={clsx(
              "absolute right-0 h-px w-[calc(50%-12px)] bg-border-light dark:bg-border-dark",
              map.horizontal,
            )}
          />
          <div
            className={clsx(
              "absolute left-0 h-px w-[calc(50%-12px)] bg-border-light dark:bg-border-dark",
              map.horizontal,
            )}
          />
        </>
      )}

      {/* Only render gradients & ticks if not simple */}
      {!simple && (
        <>
          {map.verticalTicks}
          <div
            className={clsx(
              "absolute -left-24 h-px w-24 bg-gradient-to-l from-border-light to-transparent dark:from-border-dark dark:to-transparent",
              map.side,
            )}
          />
          <div
            className={clsx(
              "absolute -right-24 h-px w-24 bg-gradient-to-r from-border-light to-transparent dark:from-border-dark dark:to-transparent",
              map.side,
            )}
          />
        </>
      )}

      {/* Dots */}
      {isThirdVariant ? (
        <>
          <div
            className={clsx(
              "absolute left-[calc(33.33%+1px)] size-2 rounded-full bg-white shadow ring-1 ring-border-light dark:bg-background-dark dark:ring-border-dark",
              map.dot,
            )}
          />
          <div
            className={clsx(
              "absolute left-[calc(66%-1px)] size-2 rounded-full bg-white shadow ring-1 ring-border-light dark:bg-background-dark dark:ring-border-dark",
              map.dot,
            )}
          />
        </>
      ) : (
        showCenterDot && (
          <div
            className={clsx(
              "absolute left-1/2 size-2 -translate-x-1/2 rounded-full bg-white shadow ring-1 ring-border-light dark:bg-background-dark dark:ring-border-dark",
              map.dot,
            )}
          />
        )
      )}
    </div>
  );
}

export function SeparatorX({ className }: { className?: string }) {
  return (
    <div
      className={clsx(
        "absolute h-[calc(100%-24px)] w-px border-l border-dashed border-border-light dark:border-border-dark",
        className,
      )}
    />
  );
}
