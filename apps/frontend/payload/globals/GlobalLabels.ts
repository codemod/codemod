import { GlobalConfig } from "payload";
import { linkField } from "../fields/shared/link";

export const GlobalLabels: GlobalConfig = {
  slug: "global-labels",
  admin: {
    group: "Site Configuration",
  },
  access: {
    read: () => true,
  },
  versions: {
    drafts: true,
  },
  fields: [
    {
      name: "internalTitle",
      type: "text",
      defaultValue: "Global Labels",
      admin: {
        hidden: true,
        description:
          "This title is only used internally, it won't be displayed on the website.",
      },
    },
    {
      name: "blog",
      type: "group",
      dbName: "b",
      label: "Blog Labels",
      fields: [
        {
          name: "relatedArticles",
          type: "text",
          label: "Related Articles Label",
          admin: {
            description:
              "Label for the related articles section shown on blog posts. Default: 'Related Articles'",
          },
        },
        {
          name: "backToIndex",
          type: "text",
          label: "Back to index",
          admin: {
            description:
              'Label for the back to index link shown on blog posts. Default: "Back to blog"',
          },
        },
      ],
    },
    {
      name: "careers",
      type: "group",
      dbName: "c",
      label: "Careers Labels",
      fields: [
        {
          name: "relatedJobs",
          type: "text",
          label: "Related positions label",
          admin: {
            description:
              "Label for the related positions section shown on job posts. Default: 'Related Positions'",
          },
        },
        {
          name: "backToIndex",
          type: "text",
          label: "Back to index",
          admin: {
            description:
              'Label for the back to index link shown on job posts. Default: "Back to careers"',
          },
        },
        {
          name: "applyToPosition",
          type: "text",
          label: "Apply to position",
          admin: {
            description:
              'Label for the apply to position link shown on job posts. Default: "Apply to position"',
          },
        },
        {
          name: "applyToPositionDescription",
          type: "text",
          label: "Apply to position description",
          admin: {
            description:
              'Label for the apply to position link shown on job posts. Default: "Ready to feel the rush?"',
          },
        },
        {
          name: "applyToPositionCTA",
          type: "text",
          label: "Apply to position CTA text",
          admin: {
            description:
              'Label for the apply to position CTA shown on job posts. Default: "Apply"',
          },
        },
      ],
    },
    {
      name: "codemodPage",
      type: "group",
      dbName: "codemod",
      label: "Codemod page",
      fields: [
        {
          name: "ogDescription",
          type: "text",
          label: "Og description",
          admin: {
            description:
              "Description for the og tag with merge fields. E.g. Explore and run {{ framework }} {{ codemod_name }} on Codemod Registry. \n Available variables: framework, codemod_name.",
          },
        },
        {
          name: "backToIndex",
          type: "text",
          label: "Back to index",
          admin: {
            description: 'Label for the back to index link. Default: "Back"',
          },
        },
        {
          name: "documentationPopup",
          type: "richText",
          label: "Documentation Popup",
          admin: {
            description:
              "Content for the documentation popup - shown upon hovering the info icon in the sidebar",
          },
        },
        {
          name: "documentationPopupLink",
          type: "group",
          dbName: "docLink",
          label: "Documentation Popup Link",
          fields: [
            {
              name: "label",
              type: "text",
              required: true,
              label: "Button label",
            },
            {
              name: "link",
              type: "text",
              required: true,
              label: "URL",
              admin: {
                description: "e.g. https://example.com or /about-page",
              },
            },
          ],
        },
        {
          name: "runSectionTitle",
          type: "text",
          label: "Run Section Title",
          admin: {
            description: 'Title for the run section. Defaults to "Run"',
          },
        },
        {
          name: "runCommandTitle",
          type: "text",
          label: "Run command title",
          admin: {
            description: 'Title for the CLI command. Defaults to "CLI"',
          },
        },
        {
          name: "runCommandPrefix",
          type: "text",
          label: "Run command prefix",
          admin: {
            description:
              'Prefix for the run command button. Defaults to "codemod"',
          },
        },
        {
          name: "vsCodeExtensionTitle",
          type: "text",
          label: "Vs code extension title",
          admin: {
            description:
              'Title for the vs code extension section. Defaults to "VS Code Extension"',
          },
        },
        {
          name: "vsCodeExtensionButtonLabel",
          type: "text",
          label: "Vs code extension button label",
          admin: {
            description:
              'Label for the vs code extension button. Defaults to "Run in VS Code"',
          },
        },
        {
          name: "codemodStudioExampleTitle",
          type: "text",
          label: "Codemod studio example title",
          admin: {
            description:
              'Title for the codemod studio example section. Defaults to "Codemod Studio Example"',
          },
        },
        {
          name: "codemodStudioExampleButtonLabel",
          type: "text",
          label: "Codemod studio example button label",
          admin: {
            description:
              'Label for the codemod studio example button. Defaults to "Run in Codemod Studio"',
          },
        },
        {
          name: "textProjectTitle",
          type: "text",
          label: "Text project title",
          admin: {
            description:
              'Title for the text project section. Defaults to "Install Text Project"',
          },
        },
        {
          name: "sourceRepoTitle",
          type: "text",
          label: "Source repo title",
          admin: {
            description:
              'Title for the source repo section. Defaults to "Repository"',
          },
        },
        {
          name: "ctaTitle",
          type: "text",
          dbName: "ctaSectionTitle",
          label: "CTA Title",
        },
        {
          name: "ctaDescription",
          type: "textarea",
          label: "CTA Description",
          admin: {
            rows: 3,
          },
        },
        {
          name: "cta",
          type: "group",
          label: "CTA",
          fields: [
            {
              name: "label",
              type: "text",
              required: true,
              label: "Button label",
            },
            {
              name: "link",
              type: "text",
              required: true,
              label: "URL",
              admin: {
                description: "e.g. https://example.com or /about-page",
              },
            },
          ],
        },
      ],
    },
  ],
};

export default GlobalLabels;
