import { CollectionConfig } from "payload/types";
import { seoField } from "../fields/shared/seo";
import { publishStatusField } from "../fields/shared/publishStatus";
import { heroSectionBlock } from "../blocks/sectionBlocks";
import {
  featuresSectionBlock,
  registrySectionBlock,
  testimonialsSectionBlock,
  fullWidthMediaSectionBlock,
} from "../blocks/sectionBlocks";
import { linkField } from "../fields/shared/link";

const Pricing: CollectionConfig = {
  slug: "pricing",
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
      name: "pathname",
      type: "text",
      required: true,
      unique: true,
      label: "Pathname",
    },
    {
      name: "internalTitle",
      type: "text",
      label: "Internal title",
    },
    {
      name: "hero",
      type: "blocks",
      label: "Hero Section",
      blocks: [heroSectionBlock],
      admin: {
        description: "Content group",
      },
    },
    {
      name: "plans",
      type: "array",
      minRows: 3,
      maxRows: 4,
      required: true,
      label: "Plans",
      admin: {
        description: "Content group",
      },
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
          name: "cta",
          type: "group",
          label: "Call to action",
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
      ],
    },
    {
      name: "sections",
      type: "blocks",
      label: "Sections",
      admin: {
        description: "Content group",
      },
      blocks: [
        featuresSectionBlock,
        registrySectionBlock,
        testimonialsSectionBlock,
        fullWidthMediaSectionBlock,
      ],
    },
    {
      name: "cta",
      type: "relationship",
      relationTo: ["page-ctas", "page-cta-doubles", "page-cta-triples"],
      label: "Page CTA (Optional)",
      admin: {
        description:
          "Call to action for a page. This is placed at the bottom of the page before the footer.",
      },
    },
    publishStatusField,
    seoField,
  ],
};

export default Pricing;
