import type { Field } from "payload";

export const seoField: Field = {
  name: "seo",
  type: "group",
  label: "SEO & social",
  admin: {
    description: "SEO and social media sharing settings",
  },
  fields: [
    {
      name: "title",
      type: "text",
      label: "Title",
      admin: {
        description: "Optional. Max 70 chars, min 15 chars",
      },
      validate: (value: string | undefined) => {
        if (value && (value.length > 70 || value.length < 15)) {
          return "Title must be between 15 and 70 characters";
        }
        return true;
      },
    },
    {
      name: "description",
      type: "textarea",
      label: "Short paragraph for SEO & social sharing (meta description)",
      admin: {
        description: "Optional. Max 160 chars, min 50 chars",
      },
      validate: (value: string | undefined) => {
        if (value && (value.length > 160 || value.length < 50)) {
          return "Description must be between 50 and 160 characters";
        }
        return true;
      },
    },
    {
      name: "image",
      type: "upload",
      relationTo: "media",
      label: "Social sharing image",
      admin: {
        description: "Optional image for social media sharing",
      },
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
