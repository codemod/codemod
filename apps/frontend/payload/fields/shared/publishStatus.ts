import type { Field } from "payload";

export const publishStatusField: Field = {
  name: "publishStatus",
  type: "select",
  label: "Search engine visibility",
  defaultValue: "public",
  required: true,
  options: [
    {
      label: "Public (will show up in Google)",
      value: "public",
    },
    {
      label: "Hidden (won't show up in Google, but accessible through URL)",
      value: "hidden",
    },
  ],
  admin: {
    description: "Control whether this page appears in search engine results",
  },
};
