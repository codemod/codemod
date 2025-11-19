import { CollectionConfig } from "payload/types";
import { seoField } from "../fields/shared/seo";
import { publishStatusField } from "../fields/shared/publishStatus";

const Jobs: CollectionConfig = {
  slug: "jobs",
  admin: {
    useAsTitle: "title",
    defaultColumns: ["title", "location", "department", "active", "updatedAt"],
  },
  access: {
    read: () => true,
  },
  versions: {
    drafts: true,
  },
  fields: [
    {
      name: "active",
      type: "checkbox",
      required: true,
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
      label: "Department",
    },
    {
      name: "post",
      type: "richText",
      required: true,
      label: "Post",
    },
    {
      name: "privacyPolicy",
      type: "group",
      required: true,
      label: "Privacy Policy",
      admin: {
        description: "Link to the privacy policy in the Job form",
      },
      fields: [
        {
          name: "label",
          type: "text",
          required: true,
          label: "Label",
          defaultValue: "Privacy Policy",
        },
        {
          name: "href",
          type: "text",
          required: true,
          label: "URL",
          defaultValue: "/privacy-policy",
        },
      ],
    },
    {
      name: "relatedPositions",
      type: "relationship",
      relationTo: "jobs",
      hasMany: true,
      label: "Related positions",
    },
    publishStatusField,
    seoField,
  ],
};

export default Jobs;
