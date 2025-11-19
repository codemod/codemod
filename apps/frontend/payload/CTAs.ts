import { CollectionConfig } from "payload";
import { styledCtaField } from "./fields/shared/styledCta";
import { linkField } from "./fields/shared/link";

// Extract fields from shared field definitions
const styledCtaFields =
  styledCtaField.type === "group" ? styledCtaField.fields : [];
const linkFields = linkField.type === "group" ? linkField.fields : [];

export const CTAs: CollectionConfig = {
  slug: "ctas",
  admin: {
    group: "Globals",
    useAsTitle: "title",
    description: "Call to action components for pages and articles",
  },
  access: {
    read: () => true,
  },
  versions: {
    drafts: true,
  },
  fields: [
    // CTA type selector
    {
      name: "ctaType",
      type: "select",
      required: true,
      label: "CTA Type",
      options: [
        { label: "Page Single", value: "page-single" },
        { label: "Page Double", value: "page-double" },
        { label: "Page Triple", value: "page-triple" },
        { label: "Article", value: "article" },
      ],
      admin: {
        description: "Select the type of CTA you want to create",
      },
    },

    // Common field: Title (required for all types)
    {
      name: "title",
      type: "text",
      required: true,
      label: "Title",
    },

    // Page Single fields
    {
      name: "paragraph",
      type: "richText",
      required: true,
      label: "Paragraph",
      admin: {
        condition: (data) =>
          data?.ctaType === "page-single" || data?.ctaType === "page-triple",
        description: "Main paragraph content",
      },
    },
    {
      name: "cta",
      type: "group",
      label: "Call to action",
      admin: {
        condition: (data) => data?.ctaType === "page-single",
      },
      fields: styledCtaFields,
    },

    // Page Double fields
    {
      name: "leftSectionTitle",
      type: "text",
      required: true,
      label: "Left section title",
      admin: {
        condition: (data) => data?.ctaType === "page-double",
      },
    },
    {
      name: "leftSectionParagraph",
      type: "richText",
      label: "Left section paragraph",
      admin: {
        condition: (data) => data?.ctaType === "page-double",
      },
    },
    {
      name: "leftSectionCta",
      type: "group",
      required: true,
      label: "Left section Call to action",
      admin: {
        condition: (data) => data?.ctaType === "page-double",
      },
      fields: styledCtaFields,
    },
    {
      name: "rightSectionTitle",
      type: "text",
      required: true,
      label: "Right section title",
      admin: {
        condition: (data) => data?.ctaType === "page-double",
      },
    },
    {
      name: "rightSectionParagraph",
      type: "richText",
      label: "Right section paragraph",
      admin: {
        condition: (data) => data?.ctaType === "page-double",
      },
    },
    {
      name: "rightSectionIsNewsletter",
      type: "checkbox",
      label: "Right section is Newsletter",
      admin: {
        condition: (data) => data?.ctaType === "page-double",
        description:
          "If true, the right section will have a newsletter form instead of a CTA",
      },
    },
    {
      name: "rightSectionCta",
      type: "group",
      label: "Right section Call to action",
      admin: {
        condition: (data) =>
          data?.ctaType === "page-double" &&
          data?.rightSectionIsNewsletter !== true,
      },
      fields: styledCtaFields,
    },
    {
      name: "privacyLink",
      type: "group",
      label: "Privacy link",
      admin: {
        condition: (data) =>
          data?.ctaType === "page-double" &&
          data?.rightSectionIsNewsletter === true,
      },
      fields: linkFields,
    },

    // Page Triple fields
    {
      name: "splitPattern",
      type: "text",
      label: "Split pattern",
      admin: {
        condition: (data) => data?.ctaType === "page-triple",
        description:
          'Creates a line break in the title. Input the characters that should preceed the line break. E.g. "."',
      },
    },
    {
      name: "ctas",
      type: "array",
      required: true,
      minRows: 3,
      maxRows: 3,
      label: "Call to actions",
      admin: {
        condition: (data) => data?.ctaType === "page-triple",
        description: "Exactly 3 CTAs required for page triple type",
      },
      fields: [
        {
          name: "cta",
          type: "group",
          fields: styledCtaFields,
        },
      ],
    },

    // Article fields
    {
      name: "subtitle",
      type: "textarea",
      label: "Subtitle",
      admin: {
        condition: (data) => data?.ctaType === "article",
        rows: 3,
      },
    },
    {
      name: "articleCta",
      type: "group",
      label: "Call to action",
      admin: {
        condition: (data) => data?.ctaType === "article",
        description: "Article CTAs use simple link format (not styled CTA)",
      },
      fields: linkFields,
    },
  ],
};

export default CTAs;
