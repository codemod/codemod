"use client";

import React from "react";
import type { GroupFieldClientComponent } from "payload";
import { RenderFields } from "@payloadcms/ui";

/**
 * Custom component for managing light and dark mode images in linked-image blocks.
 * This component works around a Payload UI bug where selecting existing images
 * causes one image to clear the other when both upload fields are in the same block.
 *
 * By rendering the nested group fields properly, we ensure
 * each upload field operates independently without state conflicts.
 */
export const LinkedImagePicker: GroupFieldClientComponent = (props) => {
  const { path, field } = props;

  // Extract the nested group fields from the field definition
  const lightModeField = (field.fields as any[])?.find(
    (f: any) => f.name === "lightMode",
  );
  const darkModeField = (field.fields as any[])?.find(
    (f: any) => f.name === "darkMode",
  );

  const basePath = path || "imageData";

  return (
    <div className="field-type linked-image-picker">
      <div
        style={{
          border: "1px solid var(--theme-elevation-200)",
          borderRadius: "4px",
          padding: "16px",
          backgroundColor: "var(--theme-elevation-50)",
        }}
      >
        <div style={{ marginBottom: "12px", fontWeight: 600 }}>Images</div>
        <div
          style={{
            display: "grid",
            gridTemplateColumns: "1fr 1fr",
            gap: "16px",
          }}
        >
          {/* Light Mode Image - Render the entire group */}
          <div>
            {lightModeField && (
              <RenderFields fields={[lightModeField]} forceRender />
            )}
          </div>

          {/* Dark Mode Image - Render the entire group */}
          <div>
            {darkModeField && (
              <RenderFields fields={[darkModeField]} forceRender />
            )}
          </div>
        </div>
        <div
          style={{
            marginTop: "12px",
            fontSize: "12px",
            color: "var(--theme-elevation-500)",
          }}
        >
          Add images for light and/or dark mode. At least one image is required.
        </div>
      </div>
    </div>
  );
};
