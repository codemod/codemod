import { CollectionConfig } from "payload/types";
import { linkField } from "../fields/shared/link";

const PageCtaDouble: CollectionConfig = {
  slug: "page-cta-doubles",
  admin: {
    useAsTitle: "title",
  },
  access: {
    read: () => true,
  },
  versions: {
    drafts: true,
  },
  fields: [
    {
      name: "title",
      type: "text",
      required: true,
      label: "Title",
    },
    {
      name: "leftSectionTitle",
      type: "text",
      required: true,
      label: "Left section title",
    },
    {
      name: "leftSectionParagraph",
      type: "richText",
      label: "Left section paragraph",
    },
    {
      name: "leftSectionCta",
      type: "group",
      required: true,
      label: "Left section Call to action",
      fields: [
        {
          name: "label",
          type: "text",
          label: "Button label",
        },
        {
          name: "link",
          type: "text",
          required: true,
          label: "Link URL",
        },
      ],
    },
    {
      name: "rightSectionTitle",
      type: "text",
      required: true,
      label: "Right section title",
    },
    {
      name: "rightSectionParagraph",
      type: "richText",
      label: "Right section paragraph",
    },
    {
      name: "rightSectionIsNewsletter",
      type: "checkbox",
      label: "Right section is Newsletter",
      admin: {
        description:
          "If true, the right section will have a newsletter form instead of a CTA",
      },
    },
    {
      name: "rightSectionCta",
      type: "group",
      label: "Right section Call to action",
      admin: {
        condition: (data) => !data?.rightSectionIsNewsletter,
      },
      fields: [
        {
          name: "label",
          type: "text",
          label: "Button label",
        },
        {
          name: "link",
          type: "text",
          required: true,
          label: "Link URL",
        },
      ],
    },
    {
      ...linkField,
      name: "privacyLink",
      label: "Privacy link",
      admin: {
        condition: (data) => data?.rightSectionIsNewsletter === true,
      },
    },
  ],
};

export default PageCtaDouble;
