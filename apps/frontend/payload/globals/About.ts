import { GlobalConfig } from "payload";
import { publishStatusField } from "../fields/shared/publishStatus";
import { heroSectionBlock } from "../blocks/sectionBlocks";
import { imageWithAltField } from "../fields/shared/imageWithAlt";

export const About: GlobalConfig = {
  slug: "about",
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
      defaultValue: "/about",
      admin: {
        readOnly: true,
        description: "Fixed pathname for the about page",
      },
    },
    {
      name: "internalTitle",
      type: "text",
      defaultValue: "About",
      admin: {
        description: "Internal title for admin use only",
      },
    },
    {
      name: "hero",
      type: "blocks",
      label: "Hero Section",
      blocks: [heroSectionBlock],
      admin: {
        description: "Hero section content",
      },
    },
    {
      name: "paragraphTitle",
      type: "text",
      required: true,
      label: "Title",
    },
    {
      name: "paragraphContent",
      type: "richText",
      required: true,
      label: "Content",
    },
    {
      name: "teamTitle",
      type: "text",
      required: true,
      label: "Team Section Title",
    },
    {
      name: "teamMembers",
      type: "array",
      label: "Team Members",
      fields: [
        {
          name: "image",
          type: "upload",
          relationTo: "media",
          label: "Image",
        },
        {
          name: "name",
          type: "text",
          required: true,
          label: "Name",
        },
        {
          name: "role",
          type: "text",
          required: true,
          label: "Role",
        },
        {
          name: "linkedin",
          type: "text",
          label: "LinkedIn Profile URL",
        },
        {
          name: "twitter",
          type: "text",
          label: "Twitter Profile URL",
        },
        {
          name: "bio",
          type: "richText",
          required: true,
          label: "Bio",
        },
        {
          name: "previousCompany",
          type: "text",
          label: "Previous Company",
        },
        {
          ...imageWithAltField,
          name: "previousCompanyLogo",
          label: "Previous Company Logo",
          admin: {
            description:
              "Please, upload logos with transparent background (svg preferred) and in their horizontal variation. Also try and trim vertical margins as much as possible.",
          },
        },
      ],
    },
    {
      name: "companies",
      type: "blocks",
      label: "Companies Section",
      blocks: [heroSectionBlock],
    },
    {
      name: "investorsTitle",
      type: "text",
      required: true,
      label: "Investors Section Title",
    },
    {
      name: "investorsSubtitle",
      type: "richText",
      required: true,
      label: "Investors Section Subtitle",
    },
    {
      name: "investors",
      type: "array",
      label: "Investors",
      fields: [
        {
          name: "image",
          type: "upload",
          relationTo: "media",
          label: "Image",
        },
        {
          name: "name",
          type: "text",
          required: true,
          label: "Name",
        },
        {
          name: "role",
          type: "text",
          required: true,
          label: "Role",
        },
        {
          ...imageWithAltField,
          name: "companyLogo",
          label: "Company Logo",
        },
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

export default About;
