import { Field } from "payload/types";

export const imageWithAltField: Field = {
  name: "imageWithAlt",
  type: "group",
  label: "Image with Light and Dark Mode Support",
  required: true,
  fields: [
    {
      name: "lightImage",
      type: "upload",
      relationTo: "media",
      required: true,
      label: "Light Mode Image",
    },
    {
      name: "darkImage",
      type: "upload",
      relationTo: "media",
      label: "Dark Mode Image",
    },
    {
      name: "alt",
      type: "text",
      required: true,
      maxLength: 150,
      label: "Descriptive label for screen readers & SEO",
      admin: {
        description:
          "Alt text should be descriptive and concise (under 150 characters).",
      },
    },
  ],
};
