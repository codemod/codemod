"use client";

import { useLiveMode } from "@sanity/react-loader";
import { VisualEditing } from "next-sanity";
import { usePathname } from "next/navigation";

import { client } from "@/data/sanity/client";
import { useEffect } from "react";

// Always enable stega in Live Mode
const stegaClient = client.withConfig({ stega: true });

export default function LiveVisualEditing() {
  useLiveMode({ client: stegaClient });
  const pathname = usePathname();

  useEffect(() => {
    if (window === parent) {
      location.href = `/api/disable-draft?redirectTo=${encodeURIComponent(
        pathname,
      )}`;
    }
  }, [pathname]);

  return (
    <>
      <VisualEditing />
      <PreviewIndicator />
    </>
  );
}

function PreviewIndicator() {
  return (
    <span className="fixed bottom-2 left-2 inline-flex items-center gap-x-1.5 rounded-md bg-pink-100 px-2 py-1 font-medium text-xs text-pink-700">
      <svg
        className="h-1.5 w-1.5 fill-pink-500"
        viewBox="0 0 6 6"
        aria-hidden="true"
      >
        <circle cx={3} cy={3} r={3} />
      </svg>
      Preview mode
    </span>
  );
}
