import { CollectionConfig } from "payload";
import { publishStatusField } from "./fields/shared/publishStatus";
import { linkField } from "./fields/shared/link";
import { formatSlug } from "./utils/formatSlug";

export const Job: CollectionConfig = {
  slug: "jobs",
  admin: {
    group: "Careers",
    useAsTitle: "title",
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
      required: true,
      unique: true,
      label: "Pathname",
      admin: {
        description:
          "URL path for this job listing. Auto-generated from title if left empty (defaults to /careers/...). You can override with any path.",
      },
      hooks: {
        beforeChange: [
          ({ value, data, operation }) => {
            // If pathname is empty and title exists, auto-generate
            if (!value && data?.title && typeof data.title === "string") {
              const slug = formatSlug(data.title);
              if (slug) {
                return `/careers/${slug}`;
              }
            }
            // Allow manual override - return whatever user entered
            return value;
          },
        ],
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

    // Job-specific fields
    {
      name: "active",
      type: "checkbox",
      required: true,
      defaultValue: false,
      label: "Active",
      admin: {
        description: "Is the job active?",
      },
    },
    {
      name: "title",
      type: "text",
      required: true,
      label: "Job title",
    },
    {
      name: "location",
      type: "text",
      required: true,
      label: "Location",
      admin: {
        description: "Location of the job",
      },
    },
    {
      name: "department",
      type: "select",
      required: true,
      label: "Department",
      options: [
        { label: "Engineering", value: "engineering" },
        { label: "Marketing", value: "marketing" },
        { label: "Sales", value: "sales" },
        { label: "Customer Success", value: "customer-success" },
        { label: "Product", value: "product" },
        { label: "Design", value: "design" },
        { label: "Finance", value: "finance" },
        { label: "People", value: "people" },
        { label: "Legal", value: "legal" },
        { label: "Operations", value: "operations" },
        { label: "Other", value: "other" },
      ],
    },
    {
      name: "post",
      type: "richText",
      required: true,
      label: "Post",
      admin: {
        description: "Job description and details",
      },
    },
    {
      ...linkField,
      name: "privacyPolicy",
      required: true,
      label: "Privacy Policy",
      admin: {
        description: "Link to the privacy policy in the Job form",
      },
      defaultValue: {
        label: "Privacy Policy",
        href: "/privacy-policy",
      },
    },
    {
      name: "relatedPositions",
      type: "relationship",
      relationTo: "jobs",
      hasMany: true,
      label: "Related positions",
    },
  ],
};

export default Job;
