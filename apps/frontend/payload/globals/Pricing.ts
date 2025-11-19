import { GlobalConfig } from "payload";
import { publishStatusField } from "../fields/shared/publishStatus";
import { heroSectionBlock } from "../blocks/sectionBlocks";
import {
  featuresSectionBlock,
  registrySectionBlock,
  testimonialsSectionBlock,
  fullWidthMediaSectionBlock,
} from "../blocks/sectionBlocks";
import { styledCtaField } from "../fields/shared/styledCta";

export const Pricing: GlobalConfig = {
  slug: "pricing",
  admin: {
    group: "Pages",
  },
  access: {
    read: () => true,
  },
  versions: {
    drafts: true,
  },
  fields: [
    {
      name: "pathname",
      type: "text",
      defaultValue: "/pricing",
      admin: {
        readOnly: true,
        description: "Fixed pathname for the pricing page",
      },
    },
    {
      name: "internalTitle",
      type: "text",
      defaultValue: "Pricing",
      admin: {
        description: "Internal title for admin use only",
      },
    },
    {
      name: "hero",
      type: "blocks",
      label: "Hero Section",
      blocks: [heroSectionBlock],
    },
    {
      name: "plans",
      type: "array",
      minRows: 3,
      maxRows: 4,
      required: true,
      label: "Plans",
      fields: [
        {
          name: "title",
          type: "text",
          required: true,
          maxLength: 80,
          label: "Title",
          admin: {
            description: "Required. Max chars: 80",
          },
        },
        {
          name: "icon",
          type: "text",
          label: "Icon",
          admin: {
            description: "Icon name/identifier",
          },
        },
        {
          name: "planDescription",
          type: "richText",
          required: true,
          label: "Plan Description",
        },
        {
          name: "price",
          type: "text",
          required: true,
          label: "Price",
        },
        {
          name: "priceNotes",
          type: "text",
          label: "Price notes (Optional)",
          admin: {
            description: "E.g. 'Starting from $99/month'",
          },
        },
        {
          name: "targetPlanDescription",
          type: "richText",
          required: true,
          label: "Target Plan Description",
          admin: {
            description: 'E.g. "For teams up to 10 developers"',
          },
        },
        {
          name: "featuresTitle",
          type: "text",
          required: true,
          label: "Features title",
        },
        {
          name: "features",
          type: "array",
          label: "Features",
          fields: [
            {
              name: "feature",
              type: "text",
              label: "Feature",
            },
          ],
        },
        {
          ...styledCtaField,
          name: "cta",
          label: "Call to action",
        },
      ],
    },
    {
      name: "sections",
      type: "blocks",
      label: "Sections",
      blocks: [
        featuresSectionBlock,
        registrySectionBlock,
        testimonialsSectionBlock,
        fullWidthMediaSectionBlock,
      ],
    },
    {
      name: "pageCta",
      type: "relationship",
      relationTo: "ctas",
      label: "Page CTA (Optional)",
      admin: {
        description:
          "Call to action for a page. This is placed at the bottom of the page before the footer.",
      },
    },
    publishStatusField,
    // SEO handled by @payloadcms/plugin-seo (adds 'meta' field automatically)
  ],
};

export default Pricing;
