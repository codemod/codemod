import { GlobalConfig } from "payload";
import { publishStatusField } from "../fields/shared/publishStatus";

export const RegistryIndex: GlobalConfig = {
  slug: "registry-index",
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
      defaultValue: "/registry",
      admin: {
        readOnly: true,
        description: "Fixed pathname for the registry index page",
      },
    },
    {
      name: "internalTitle",
      type: "text",
      defaultValue: "Registry Index",
      admin: {
        description: "Internal title for admin use only",
      },
    },
    publishStatusField,
    // SEO handled by @payloadcms/plugin-seo (adds 'meta' field automatically)
    {
      name: "placeholders",
      type: "group",
      label: "Placeholder Text",
      fields: [
        {
          name: "emptyStateText",
          type: "textarea",
          label: "Empty state text",
          admin: {
            rows: 3,
          },
        },
        {
          name: "searchPlaceholder",
          type: "text",
          label: "Search placeholder",
          admin: {
            description:
              "Main search input's placeholder text. Defaults to 'Search for codemods'.",
          },
        },
        {
          name: "totalCodemodsSuffix",
          type: "text",
          label: "Total codemods suffix",
          admin: {
            description:
              'Text to display after the total number of codemods. Displays next to the search bar. Defaults to "automations found".',
          },
        },
        {
          name: "verifiedAutomationTooltip",
          type: "textarea",
          label: "Verified Automation Tooltip",
          admin: {
            description:
              "Tooltip text for the verified automation badge. Keep below 150 characters.",
            rows: 3,
          },
        },
      ],
    },
  ],
};

export default RegistryIndex;
