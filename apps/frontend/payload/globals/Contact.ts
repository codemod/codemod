import { GlobalConfig } from "payload";
import { publishStatusField } from "../fields/shared/publishStatus";

export const Contact: GlobalConfig = {
  slug: "contact",
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
    // Page fields (from definePage)
    {
      name: "pathname",
      type: "text",
      defaultValue: "/contact",
      admin: {
        readOnly: true,
        description: "Fixed pathname for the contact page",
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

    // Contact Page-specific fields
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
      name: "formFields",
      type: "group",
      required: true,
      label: "Form Fields",
      admin: {
        description: "The labels and placeholders for the form fields",
      },
      fields: [
        {
          name: "name",
          type: "text",
          required: true,
          label: "Name field label",
          admin: {
            description: "The label for the name field",
          },
        },
        {
          name: "namePlaceholder",
          type: "text",
          required: true,
          label: "Name field placeholder",
          admin: {
            description: "The placeholder for the name field",
          },
        },
        {
          name: "email",
          type: "text",
          required: true,
          label: "Email field label",
          admin: {
            description: "The label for the email field",
          },
        },
        {
          name: "emailPlaceholder",
          type: "text",
          required: true,
          label: "Email field placeholder",
          admin: {
            description: "The placeholder for the email field",
          },
        },
        {
          name: "company",
          type: "text",
          required: true,
          label: "Company field label",
          admin: {
            description: "The label for the company field",
          },
        },
        {
          name: "companyPlaceholder",
          type: "text",
          required: true,
          label: "Company field placeholder",
          admin: {
            description: "The placeholder for the company field",
          },
        },
        {
          name: "message",
          type: "text",
          required: true,
          label: "Message field label",
          admin: {
            description: "The label for the message field",
          },
        },
        {
          name: "messagePlaceholder",
          type: "text",
          required: true,
          label: "Message field placeholder",
          admin: {
            description: "The placeholder for the message field",
          },
        },
        {
          name: "privacy",
          type: "text",
          required: true,
          label: "Privacy checkbox label",
          admin: {
            description: "The label for the privacy policy checkbox",
          },
        },
        {
          name: "privacyLabel",
          type: "text",
          required: true,
          label: "Privacy link label",
          admin: {
            description: "The label for the privacy policy link",
          },
        },
        {
          name: "privacyLink",
          type: "text",
          required: true,
          label: "Privacy policy link",
          admin: {
            description: "The privacy policy link",
          },
        },
        {
          name: "submit",
          type: "text",
          required: true,
          label: "Submit button label",
          admin: {
            description: "The label for the submit button",
          },
        },
      ],
    },
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

export default Contact;
