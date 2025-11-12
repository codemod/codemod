import type { GHBranch } from "@/types/github";
import { faker } from "@faker-js/faker";
import { Factory } from "miragejs";

export const branchFactory = Factory.extend<Omit<GHBranch, "id">>({
  name() {
    return faker.company.name();
  },
  commit() {
    return {
      sha: faker.datatype.uuid(),
      url: faker.internet.url(),
    };
  },
  protected() {
    return false;
  },
  protection() {
    return {
      enabled: false,
      required_status_checks: {
        enforcement_level: "",
        contexts: [],
      },
    };
  },
});
