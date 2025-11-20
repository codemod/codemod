"use client";

import React, { useState, useCallback } from "react";
import type { TextFieldClientComponent } from "payload";
import { useField } from "@payloadcms/ui";
import { IconId } from "@/components/shared/Icon";
import Icon from "@/components/shared/Icon";

// Convert IconId enum to array of { label, value }
const iconOptions = Object.entries(IconId).map(([label, value]) => ({
  label: label.replace(/([A-Z])/g, " $1").trim(),
  value: value as string,
}));

export const IconPicker: TextFieldClientComponent = (props) => {
  const { path } = props;

  // Use Payload's useField hook - it automatically gets the field from context
  const { value, setValue } = useField<string>({ path: path || "icon" });

  const [isOpen, setIsOpen] = useState(false);
  const [search, setSearch] = useState("");

  const filteredIcons = search
    ? iconOptions.filter(
        (icon) =>
          icon.label.toLowerCase().includes(search.toLowerCase()) ||
          icon.value.toLowerCase().includes(search.toLowerCase()),
      )
    : iconOptions;

  const handleSelect = useCallback(
    (iconValue: string) => {
      setValue(iconValue);
      setIsOpen(false);
      setSearch("");
    },
    [setValue],
  );

  const handleClear = useCallback(
    (e: React.MouseEvent) => {
      e.stopPropagation();
      e.preventDefault();
      setValue("");
    },
    [setValue],
  );

  // Check if value exists (can be string or empty string)
  const selectedIcon =
    value && typeof value === "string" && value.trim()
      ? iconOptions.find((opt) => opt.value === value)
      : null;

  return (
    <div className="field-type icon-picker">
      <div
        onClick={() => setIsOpen(true)}
        style={{
          padding: "12px",
          border: "1px solid var(--theme-elevation-200)",
          borderRadius: "4px",
          cursor: "pointer",
          backgroundColor: "var(--theme-elevation-50)",
          display: "flex",
          alignItems: "center",
          gap: "12px",
          position: "relative",
        }}
      >
        {selectedIcon ? (
          <>
            <div
              style={{
                backgroundColor: "white",
                color: "black",
                padding: "4px",
                borderRadius: "4px",
                width: "36px",
                height: "36px",
                display: "flex",
                alignItems: "center",
                justifyContent: "center",
                flexShrink: 0,
              }}
            >
              <Icon name={selectedIcon.value as any} />
            </div>
            <span style={{ flex: 1 }}>{selectedIcon.label}</span>
            <button
              onClick={handleClear}
              type="button"
              style={{
                padding: "4px 8px",
                border: "1px solid var(--theme-elevation-200)",
                borderRadius: "4px",
                backgroundColor: "var(--theme-elevation-100)",
                cursor: "pointer",
                fontSize: "12px",
                color: "var(--theme-elevation-800)",
              }}
              onMouseEnter={(e) => {
                e.currentTarget.style.backgroundColor =
                  "var(--theme-elevation-200)";
              }}
              onMouseLeave={(e) => {
                e.currentTarget.style.backgroundColor =
                  "var(--theme-elevation-100)";
              }}
            >
              Clear
            </button>
          </>
        ) : (
          <span style={{ color: "var(--theme-elevation-400)", flex: 1 }}>
            Click to select an icon
          </span>
        )}
      </div>

      {isOpen && (
        <div
          style={{
            position: "fixed",
            top: 0,
            left: 0,
            right: 0,
            bottom: 0,
            backgroundColor: "rgba(0, 0, 0, 0.5)",
            zIndex: 1000,
            display: "flex",
            alignItems: "center",
            justifyContent: "center",
            padding: "20px",
          }}
          onClick={() => {
            setIsOpen(false);
            setSearch("");
          }}
        >
          <div
            style={{
              backgroundColor: "var(--theme-elevation-0)",
              borderRadius: "8px",
              padding: "24px",
              maxWidth: "900px",
              width: "100%",
              maxHeight: "80vh",
              overflow: "auto",
              boxShadow: "0 4px 6px rgba(0, 0, 0, 0.1)",
            }}
            onClick={(e) => e.stopPropagation()}
          >
            <div
              style={{
                display: "flex",
                justifyContent: "space-between",
                alignItems: "center",
                marginBottom: "16px",
              }}
            >
              <h2 style={{ margin: 0 }}>Select Icon</h2>
              <button
                onClick={() => {
                  setIsOpen(false);
                  setSearch("");
                }}
                type="button"
                style={{
                  padding: "8px 16px",
                  border: "1px solid var(--theme-elevation-200)",
                  borderRadius: "4px",
                  backgroundColor: "var(--theme-elevation-100)",
                  cursor: "pointer",
                }}
              >
                Close
              </button>
            </div>
            <input
              type="text"
              placeholder="Search for an icon..."
              value={search}
              onChange={(e) => setSearch(e.target.value)}
              style={{
                width: "100%",
                padding: "8px 12px",
                marginBottom: "16px",
                border: "1px solid var(--theme-elevation-200)",
                borderRadius: "4px",
                fontSize: "14px",
              }}
              autoFocus
            />
            <div
              style={{
                display: "grid",
                gridTemplateColumns: "repeat(auto-fill, minmax(120px, 1fr))",
                gap: "12px",
              }}
            >
              {filteredIcons.map((icon) => (
                <div
                  key={icon.value}
                  onClick={() => handleSelect(icon.value)}
                  style={{
                    padding: "16px",
                    border:
                      selectedIcon?.value === icon.value
                        ? "2px solid var(--theme-elevation-400)"
                        : "1px solid var(--theme-elevation-200)",
                    borderRadius: "4px",
                    cursor: "pointer",
                    textAlign: "center",
                    transition: "all 0.2s",
                    backgroundColor:
                      selectedIcon?.value === icon.value
                        ? "var(--theme-elevation-50)"
                        : "transparent",
                  }}
                  onMouseEnter={(e) => {
                    if (selectedIcon?.value !== icon.value) {
                      e.currentTarget.style.backgroundColor =
                        "var(--theme-elevation-50)";
                    }
                  }}
                  onMouseLeave={(e) => {
                    if (selectedIcon?.value !== icon.value) {
                      e.currentTarget.style.backgroundColor = "transparent";
                    }
                  }}
                >
                  <div
                    style={{
                      backgroundColor: "white",
                      color: "black",
                      padding: "8px",
                      borderRadius: "4px",
                      width: "48px",
                      height: "48px",
                      display: "flex",
                      alignItems: "center",
                      justifyContent: "center",
                      margin: "0 auto 8px",
                    }}
                  >
                    <Icon name={icon.value as any} />
                  </div>
                  <div
                    style={{
                      fontSize: "12px",
                      color: "var(--theme-elevation-600)",
                      fontWeight:
                        selectedIcon?.value === icon.value ? 600 : 400,
                    }}
                  >
                    {icon.label}
                  </div>
                </div>
              ))}
            </div>
            {filteredIcons.length === 0 && (
              <div
                style={{
                  textAlign: "center",
                  padding: "40px",
                  color: "var(--theme-elevation-400)",
                }}
              >
                No icons found matching "{search}"
              </div>
            )}
          </div>
        </div>
      )}
    </div>
  );
};
