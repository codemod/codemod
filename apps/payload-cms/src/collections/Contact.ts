import { CollectionConfig } from "payload/types";
import { seoField } from "../fields/shared/seo";
import { publishStatusField } from "../fields/shared/publishStatus";

const Contact: CollectionConfig = {
  slug: "contact",
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
      admin: {
        description: "The title of the page",
      },
    },
    {
      name: "description",
      type: "text",
      required: true,
      label: "Description",
      admin: {
        description: "The description shown below the title",
      },
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
      name: "formFields",
      type: "group",
      required: true,
      label: "The labels and placeholders for the form fields",
      fields: [
        {
          name: "name",
          type: "text",
          required: true,
          label: "The label for the name field",
        },
        {
          name: "namePlaceholder",
          type: "text",
          required: true,
          label: "The placeholder for the name field",
        },
        {
          name: "email",
          type: "text",
          required: true,
          label: "The label for the email field",
        },
        {
          name: "emailPlaceholder",
          type: "text",
          required: true,
          label: "The placeholder for the email field",
        },
        {
          name: "company",
          type: "text",
          required: true,
          label: "The label for the company field",
        },
        {
          name: "companyPlaceholder",
          type: "text",
          required: true,
          label: "The placeholder for the company field",
        },
        {
          name: "message",
          type: "text",
          required: true,
          label: "The label for the message field",
        },
        {
          name: "messagePlaceholder",
          type: "text",
          required: true,
          label: "The placeholder for the message field",
        },
        {
          name: "privacy",
          type: "text",
          required: true,
          label: "The label for the privacy policy checkbox",
        },
        {
          name: "privacyLabel",
          type: "text",
          required: true,
          label: "The label for the privacy policy link",
        },
        {
          name: "privacyLink",
          type: "text",
          required: true,
          label: "The privacy policy link",
        },
        {
          name: "submit",
          type: "text",
          required: true,
          label: "The label for the submit button",
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

export default Contact;
