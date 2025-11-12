import WButton from "@/components/shared/Button";
import { SignedIn, SignedOut, UserButton } from "@clerk/nextjs";
import { SignIn as SignInIcon } from "@phosphor-icons/react";

import { useRouter } from "next/navigation";

const AuthButtons = ({
  variant = "www",
  redirectUrl,
}: {
  variant: "studio" | "www";
  redirectUrl: string;
}) => {
  const router = useRouter();

  const signUserIn = () => {
    const queryParams = new URLSearchParams({ variant }).toString();
    router.push(`/auth/sign-in?${queryParams}`);
  };

  const Button = WButton;
  return (
    <>
      <SignedOut>
        <Button onClick={signUserIn} intent="inline">
          {variant === "studio" && <SignInIcon className="mr-2 h-4 w-4" />}
          Sign in
        </Button>
      </SignedOut>
      <SignedIn>
        <div className="flex items-center gap-2">
          <UserButton afterSignOutUrl={redirectUrl} />
        </div>
      </SignedIn>
    </>
  );
};

export default AuthButtons;
