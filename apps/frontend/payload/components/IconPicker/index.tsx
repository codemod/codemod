"use client";

import React, { useState, useCallback } from "react";
import type { TextFieldClientComponent } from "payload";
import { IconId } from "@/components/shared/Icon";
import Icon from "@/components/shared/Icon";

// Convert IconId enum to array of { label, value }
const iconOptions = Object.entries(IconId).map(([label, value]) => ({
  label: label.replace(/([A-Z])/g, " $1").trim(),
  value: value as string,
}));

export const IconPicker: TextFieldClientComponent = (props) => {
  const { value, onChange } = props;
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
      if (onChange) {
        onChange(iconValue);
      }
      setIsOpen(false);
      setSearch("");
    },
    [onChange],
  );

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
        }}
      >
        {value ? (
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
              }}
            >
              <Icon name={value as any} />
            </div>
            <span>
              {iconOptions.find((opt) => opt.value === value)?.label || value}
            </span>
          </>
        ) : (
          <span style={{ color: "var(--theme-elevation-400)" }}>
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
              maxWidth: "800px",
              maxHeight: "80vh",
              overflow: "auto",
              width: "90%",
            }}
            onClick={(e) => e.stopPropagation()}
          >
            <h2 style={{ marginTop: 0, marginBottom: "16px" }}>Select Icon</h2>
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
              }}
            />
            <div
              style={{
                display: "grid",
                gridTemplateColumns: "repeat(auto-fill, minmax(100px, 1fr))",
                gap: "12px",
              }}
            >
              {filteredIcons.map((icon) => (
                <div
                  key={icon.value}
                  onClick={() => handleSelect(icon.value)}
                  style={{
                    padding: "12px",
                    border: "1px solid var(--theme-elevation-200)",
                    borderRadius: "4px",
                    cursor: "pointer",
                    textAlign: "center",
                    transition: "background-color 0.2s",
                  }}
                  onMouseEnter={(e) => {
                    e.currentTarget.style.backgroundColor =
                      "var(--theme-elevation-50)";
                  }}
                  onMouseLeave={(e) => {
                    e.currentTarget.style.backgroundColor = "transparent";
                  }}
                >
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
                      margin: "0 auto 8px",
                    }}
                  >
                    <Icon name={icon.value as any} />
                  </div>
                  <div
                    style={{
                      fontSize: "12px",
                      color: "var(--theme-elevation-600)",
                    }}
                  >
                    {icon.label}
                  </div>
                </div>
              ))}
            </div>
          </div>
        </div>
      )}
    </div>
  );
};
