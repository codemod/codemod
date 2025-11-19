import { Field } from "payload/types";

export const publishStatusField: Field = {
  name: "publishStatus",
  type: "select",
  label: "Search engine visibility",
  required: true,
  defaultValue: "public",
  options: [
    { label: "Public", value: "public" },
    { label: "No Index", value: "noIndex" },
  ],
};
