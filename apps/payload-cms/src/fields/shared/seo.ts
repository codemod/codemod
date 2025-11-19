import { Field } from "payload/types";

export const seoField: Field = {
  name: "seo",
  type: "group",
  label: "SEO & social",
  fields: [
    {
      name: "title",
      type: "text",
      label: "Title",
      admin: {
        description: "Optional",
      },
      validate: (value: string) => {
        if (value && (value.length < 15 || value.length > 70)) {
          return "Title should be between 15 and 70 characters";
        }
        return true;
      },
    },
    {
      name: "description",
      type: "textarea",
      label: "Short paragraph for SEO & social sharing (meta description)",
      admin: {
        description: "Optional",
      },
      validate: (value: string) => {
        if (value && (value.length < 50 || value.length > 160)) {
          return "Description should be between 50 and 160 characters";
        }
        return true;
      },
    },
    {
      name: "image",
      type: "upload",
      relationTo: "media",
      label: "Social sharing image",
    },
    {
      name: "canonicalUrl",
      type: "text",
      label: "Custom canonical URL",
      admin: {
        description:
          "Optional. Use this in case the content of this page is duplicated elsewhere and you'd like to point search engines to that other URL instead",
      },
    },
  ],
};
