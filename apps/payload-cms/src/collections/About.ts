import { CollectionConfig } from "payload/types";
import { seoField } from "../fields/shared/seo";
import { publishStatusField } from "../fields/shared/publishStatus";
import { heroSectionBlock } from "../blocks/sectionBlocks";
import { imageWithAltField } from "../fields/shared/imageWithAlt";

const About: CollectionConfig = {
  slug: "about",
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
      name: "paragraphTitle",
      type: "text",
      required: true,
      label: "Title",
      admin: {
        description: "Content group",
      },
    },
    {
      name: "paragraphContent",
      type: "richText",
      required: true,
      label: "Content",
      admin: {
        description: "Content group",
      },
    },
    {
      name: "teamTitle",
      type: "text",
      required: true,
      label: "Team Section Title",
      admin: {
        description: "Content group",
      },
    },
    {
      name: "teamMembers",
      type: "array",
      label: "Team Members",
      admin: {
        description: "Content group",
      },
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
      admin: {
        description: "Content group",
      },
    },
    {
      name: "investorsTitle",
      type: "text",
      required: true,
      label: "Investors Section Title",
      admin: {
        description: "Content group",
      },
    },
    {
      name: "investorsSubtitle",
      type: "richText",
      required: true,
      label: "Investors Section Subtitle",
      admin: {
        description: "Content group",
      },
    },
    {
      name: "investors",
      type: "array",
      label: "Investors",
      admin: {
        description: "Content group",
      },
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

export default About;
