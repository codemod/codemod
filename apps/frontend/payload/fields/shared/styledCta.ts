import type { Field } from "payload";

export const styledCtaField: Field = {
  name: "styledCta",
  type: "group",
  label: "Styled CTA",
  fields: [
    {
      name: "title",
      type: "text",
      label: "Title",
      admin: {
        description: "The title of the CTA (Optional)",
      },
    },
    {
      name: "label",
      type: "text",
      label: "Button label",
    },
    {
      name: "link",
      type: "text",
      required: true,
      label: "Link",
      admin: {
        description: "URL for the CTA button",
      },
    },
    {
      name: "style",
      type: "select",
      dbName: "s",
      label: "Style",
      options: [
        { label: "Primary", value: "primary" },
        { label: "Secondary", value: "secondary" },
      ],
      defaultValue: "primary",
    },
    {
      name: "icon",
      type: "text",
      label: "Icon",
      admin: {
        description: "Select an icon from the icon library",
        components: {
          Field: "@/payload/components/IconPicker#IconPicker",
        },
      },
    },
  ],
};
