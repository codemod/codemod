import { CollectionConfig } from "payload/types";
import { seoField } from "../fields/shared/seo";
import { publishStatusField } from "../fields/shared/publishStatus";

const Careers: CollectionConfig = {
  slug: "careers",
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
      label: "Page title",
    },
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
      name: "subtitle",
      type: "richText",
      label: "Subtitle",
    },
    {
      name: "jobs",
      type: "relationship",
      relationTo: "jobs",
      hasMany: true,
      label: "Jobs",
      admin: {
        description: "Active jobs will be automatically filtered",
      },
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

export default Careers;
