import { CollectionConfig } from "payload/types";
import { seoField } from "../fields/shared/seo";
import { publishStatusField } from "../fields/shared/publishStatus";

const RegistryIndex: CollectionConfig = {
  slug: "registry-index",
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
      label: "Title",
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
      name: "placeholders",
      type: "group",
      label: "Placeholder Text",
      admin: {
        description: "Content group",
      },
      fields: [
        {
          name: "emptyStateText",
          type: "textarea",
          label: "Empty state text",
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
          maxLength: 150,
          label: "Verified Automation Tooltip",
          admin: {
            description:
              "Tooltip text for the verified automation badge. Keep below 150 characters.",
          },
        },
      ],
    },
    publishStatusField,
    seoField,
  ],
};

export default RegistryIndex;
