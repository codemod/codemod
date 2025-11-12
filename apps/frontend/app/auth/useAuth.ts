import { useAuth as useClerk } from "@clerk/nextjs";
import { useRouter } from "next/navigation";

export const useAuth = () => {
  const router = useRouter();
  const clerk = useClerk();
  return {
    ...clerk,
    getToken: clerk.getToken,
    getSignIn: () => () => {
      router.push("/auth/sign-in");
    },
  };
};
