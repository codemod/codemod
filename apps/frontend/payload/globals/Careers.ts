import { GlobalConfig } from "payload";
import { publishStatusField } from "../fields/shared/publishStatus";

export const Careers: GlobalConfig = {
  slug: "careers",
  admin: {
    group: "Careers",
  },
  access: {
    read: () => true,
  },
  versions: {
    drafts: true,
  },
  fields: [
    // Page fields (from definePage)
    {
      name: "pathname",
      type: "text",
      defaultValue: "/careers",
      admin: {
        readOnly: true,
        description: "Fixed pathname for the careers page",
      },
    },
    {
      name: "internalTitle",
      type: "text",
      label: "Internal title",
      admin: {
        description:
          "This title is only used internally in Payload, it won't be displayed on the website.",
      },
    },
    publishStatusField,
    // SEO handled by @payloadcms/plugin-seo (adds 'meta' field automatically)

    // Careers Page-specific fields (matching Sanity schema)
    {
      name: "title",
      type: "text",
      required: true,
      label: "Page title",
      admin: {
        description: "Main title for the careers page",
      },
    },
    {
      name: "subtitle",
      type: "richText",
      label: "Subtitle",
      admin: {
        description: "Subtitle content displayed below the main title",
      },
    },
    // Note: Jobs are NOT stored here - they are queried separately from the jobs collection
    // Jobs are filtered by active status when rendering the careers page
    {
      name: "cta",
      type: "relationship",
      relationTo: "ctas",
      label: "Page CTA (Optional)",
      admin: {
        description:
          "Call to action for a page. This is placed at the bottom of the page before the footer.",
      },
    },
  ],
};

export default Careers;
