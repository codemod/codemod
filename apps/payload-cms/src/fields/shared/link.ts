import { Field } from "payload/types";

export const linkField: Field = {
  name: "link",
  type: "group",
  label: "Link",
  fields: [
    {
      name: "label",
      type: "text",
      required: true,
      label: "Label",
    },
    {
      name: "href",
      type: "text",
      required: true,
      label: "URL",
      admin: {
        description: "e.g. https://example.com or /about-page",
      },
    },
  ],
};
