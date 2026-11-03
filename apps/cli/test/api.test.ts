import type {
  CodemodDownloadLinkResponse,
  CodemodListResponse,
  CreateAPIKeyRequest,
  CreateAPIKeyResponse,
  DeleteAPIKeysRequest,
  GetCodemodResponse,
  GetUserDataResponse,
  ListAPIKeysResponse,
} from "@codemod-com/api-types";
import { Octokit } from "@octokit/rest";
import axios from "axios";
import type { Mocked, MockedClass } from "vitest";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import {
  confirmUserLoggedIn,
  createAPIKey,
  createCodeDiff,
  deleteAPIKeys,
  extractPrintableApiError,
  generateUserLoginIntent,
  getCLIAccessToken,
  getCodemod,
  getCodemodDownloadURI,
  getCodemodList,
  getGithubAPIKey,
  getGithubAvailableScope,
  getUserData,
  listAPIKeys,
  publish,
  revokeCLIToken,
  unpublish,
  validateCLIToken,
} from "../src/api.js";

// Mock axios
vi.mock("axios");
const mockedAxios = axios as Mocked<typeof axios>;

// Mock octokit
vi.mock("@octokit/rest");
const mockedOctokit = Octokit as MockedClass<typeof Octokit>;

describe("API Client Tests", () => {
  const mockAccessToken = "test-access-token";
  const mockApiKey = "test-api-key";

  beforeEach(() => {
    // Clear all mocks before each test
    vi.clearAllMocks();

    // Reset process.env
    process.env.BACKEND_URL = "http://api.example.com";
    process.env.AUTH_BACKEND_URL = "http://auth.example.com";
  });

  afterEach(() => {
    vi.resetAllMocks();
  });

  describe("extractPrintableApiError", () => {
    it("should return error message for Error", () => {
      const mockError = new Error("Network error");

      expect(extractPrintableApiError(mockError)).toBe("Network error");
    });

    it("should return default message for unknown error", () => {
      expect(extractPrintableApiError({})).toBe("An unknown error occurred.");
    });
  });

  describe("createAPIKey", () => {
    const mockRequest: CreateAPIKeyRequest = {
      name: "Test API Key",
      expiresAt: "3600",
    };

    const mockResponse: CreateAPIKeyResponse = {
      uuid: "test-uuid",
      key: "test-key-123",
    };

    it("should successfully create an API key", async () => {
      // Setup mock
      mockedAxios.post.mockResolvedValueOnce({ data: mockResponse });

      // Execute
      const result = await createAPIKey(mockAccessToken, mockRequest);

      // Verify
      expect(mockedAxios.post).toHaveBeenCalledWith(
        "http://api.example.com/api-keys",
        mockRequest,
        {
          headers: { Authorization: `Bearer ${mockAccessToken}` },
          timeout: 10000,
        },
      );
      expect(result).toEqual(mockResponse);
    });

    it("should handle API errors correctly", async () => {
      // Setup mock for error case
      const errorMessage = "API Key creation failed";
      mockedAxios.post.mockRejectedValueOnce(new Error(errorMessage));

      // Execute and verify
      await expect(createAPIKey(mockAccessToken, mockRequest)).rejects.toThrow(
        errorMessage,
      );
    });
  });

  describe("listAPIKeys", () => {
    const mockResponse: ListAPIKeysResponse = {
      keys: [
        {
          uuid: "key-1",
          name: "API Key 1",
          start: "test-start",
          createdAt: new Date().getTime(),
          expiresAt: new Date().getTime() + 3600,
        },
      ],
    };

    it("should successfully list API keys", async () => {
      // Setup mock
      mockedAxios.get.mockResolvedValueOnce({ data: mockResponse });

      // Execute
      const result = await listAPIKeys(mockAccessToken);

      // Verify
      expect(mockedAxios.get).toHaveBeenCalledWith(
        "http://api.example.com/api-keys",
        {
          headers: { Authorization: `Bearer ${mockAccessToken}` },
          timeout: 10000,
        },
      );
      expect(result).toEqual(mockResponse);
    });
  });

  describe("getUserData", () => {
    const mockUserResponse: GetUserDataResponse = {
      allowedNamespaces: [],
      organizations: [],
      user: {
        id: "user-id",
        passwordEnabled: true,
        totpEnabled: false,
        backupCodeEnabled: false,
        twoFactorEnabled: false,
        banned: false,
        createdAt: new Date().getTime(),
        updatedAt: new Date().getTime(),
        imageUrl: "https://example.com/avatar.jpg",
        hasImage: true,
        primaryEmailAddressId: "email-id",
        primaryPhoneNumberId: null,
        primaryWeb3WalletId: null,
        lastSignInAt: null,
        externalId: null,
        username: "testuser",
        firstName: "Test",
        lastName: "User",
        publicMetadata: {},
        privateMetadata: {},
        unsafeMetadata: {},
        emailAddresses: [],
        phoneNumbers: [],
        web3Wallets: [],
        externalAccounts: [],
        samlAccounts: [],
        lastActiveAt: null,
        createOrganizationEnabled: false,
        fullName: null,
        primaryEmailAddress: null,
        primaryPhoneNumber: null,
        primaryWeb3Wallet: null,
        locked: false,
        createOrganizationsLimit: null,
        deleteSelfEnabled: false,
        legalAcceptedAt: null,
      },
    };

    it("should return user data when request succeeds", async () => {
      mockedAxios.get.mockResolvedValueOnce({ data: mockUserResponse });

      const result = await getUserData(mockAccessToken);

      expect(mockedAxios.get).toHaveBeenCalledWith(
        "http://auth.example.com/userData",
        {
          headers: { Authorization: `Bearer ${mockAccessToken}` },
          timeout: 5000,
        },
      );
      expect(result).toEqual(mockUserResponse);
    });

    it("should return null when response does not contain user data", async () => {
      mockedAxios.get.mockResolvedValueOnce({ data: {} });

      const result = await getUserData(mockAccessToken);

      expect(result).toBeNull();
    });

    it("should return null when request fails", async () => {
      mockedAxios.get.mockRejectedValueOnce(new Error("Network error"));

      const result = await getUserData(mockAccessToken);

      expect(result).toBeNull();
    });
  });

  describe("validateCLIToken", () => {
    it("should validate the CLI token", async () => {
      const mockResponse = { valid: true };

      mockedAxios.get.mockResolvedValueOnce({ data: mockResponse });

      const result = await validateCLIToken(mockAccessToken);

      expect(mockedAxios.get).toHaveBeenCalledWith(
        "http://auth.example.com/verifyToken",
        {
          headers: { Authorization: `Bearer ${mockAccessToken}` },
          timeout: 5000,
        },
      );
      expect(result).toEqual(mockResponse);
    });
  });

  describe("getCLIAccessToken", () => {
    it("should get the CLI access token", async () => {
      const mockResponse = { token: "test" };

      mockedAxios.get.mockResolvedValueOnce({ data: mockResponse });

      expect(await getCLIAccessToken(mockAccessToken)).toEqual(mockResponse);
    });
  });

  describe("deleteAPIKeys", () => {
    const mockRequest: DeleteAPIKeysRequest = {
      uuid: "test-uuid",
    };

    it("should successfully delete API keys", async () => {
      mockedAxios.delete.mockResolvedValueOnce({ data: {} });

      await deleteAPIKeys(mockAccessToken, mockRequest);

      expect(mockedAxios.delete).toHaveBeenCalledWith(
        `http://api.example.com/api-keys/${mockRequest.uuid}`,
        {
          headers: { Authorization: `Bearer ${mockAccessToken}` },
          timeout: 10000,
        },
      );
    });

    it("should handle delete API keys error", async () => {
      const errorMessage = "Failed to delete API keys";
      mockedAxios.delete.mockRejectedValueOnce(new Error(errorMessage));

      await expect(deleteAPIKeys(mockAccessToken, mockRequest)).rejects.toThrow(
        errorMessage,
      );
    });
  });

  describe("publish", () => {
    const mockFormData = new FormData();

    it("should publish with access token", async () => {
      mockedAxios.post.mockResolvedValueOnce({ data: {} });

      await publish({ accessToken: mockAccessToken }, mockFormData);

      expect(mockedAxios.post).toHaveBeenCalledWith(
        "http://api.example.com/publish",
        mockFormData,
        {
          headers: {
            "Content-Type": "multipart/form-data",
            Authorization: `Bearer ${mockAccessToken}`,
          },
          timeout: 10000,
        },
      );
    });

    it("should publish with API key", async () => {
      mockedAxios.post.mockResolvedValueOnce({ data: {} });

      await publish({ apiKey: mockApiKey }, mockFormData);

      expect(mockedAxios.post).toHaveBeenCalledWith(
        "http://api.example.com/publish",
        mockFormData,
        {
          headers: {
            "Content-Type": "multipart/form-data",
            "X-API-Key": mockApiKey,
          },
          timeout: 10000,
        },
      );
    });
  });

  describe("unpublish", () => {
    const mockName = "test-codemod";

    it("should successfully unpublish", async () => {
      mockedAxios.post.mockResolvedValueOnce({ data: {} });

      await unpublish(mockAccessToken, mockName);

      expect(mockedAxios.post).toHaveBeenCalledWith(
        "http://api.example.com/unpublish",
        { name: mockName },
        {
          headers: { Authorization: `Bearer ${mockAccessToken}` },
          timeout: 10000,
        },
      );
    });
  });

  describe("revokeCLIToken", () => {
    it("should successfully revoke CLI token", async () => {
      mockedAxios.delete.mockResolvedValueOnce({ data: {} });

      await revokeCLIToken(mockAccessToken);

      expect(mockedAxios.delete).toHaveBeenCalledWith(
        "http://auth.example.com/revokeToken",
        {
          headers: { Authorization: `Bearer ${mockAccessToken}` },
          timeout: 10000,
        },
      );
    });
  });

  describe("getCodemod", () => {
    const mockName = "test-codemod";
    const mockResponse: GetCodemodResponse = {
      name: mockName,
      arguments: [],
      amountOfUses: 0,
      applicability: null,
      author: "test-author",
      createdAt: new Date(),
      engine: null,
      featured: false,
      frameworks: [],
      frameworkVersion: null,
      id: 3003,
      hidden: false,
      labels: [],
      openedPrs: 0,
      private: false,
      shortDescription: "Test codemod",
      slug: "test-codemod",
      tags: [],
      updatedAt: new Date(),
      totalRuns: 0,
      verified: false,
      totalTimeSaved: 0,
      useCaseCategory: null,
      versions: [],
    };

    it("should get codemod with access token", async () => {
      mockedAxios.get.mockResolvedValueOnce({ data: mockResponse });

      const result = await getCodemod(mockName, {
        accessToken: mockAccessToken,
      });

      expect(mockedAxios.get).toHaveBeenCalledWith(
        `http://api.example.com/codemods/${mockName}`,
        {
          headers: { Authorization: `Bearer ${mockAccessToken}` },
          timeout: 10000,
        },
      );
      expect(result).toEqual(mockResponse);
    });

    it("should get codemod with API key", async () => {
      mockedAxios.get.mockResolvedValueOnce({ data: mockResponse });

      const result = await getCodemod(mockName, { apiKey: mockApiKey });

      expect(mockedAxios.get).toHaveBeenCalledWith(
        `http://api.example.com/codemods/${mockName}`,
        {
          headers: { "X-API-Key": mockApiKey },
          timeout: 10000,
        },
      );
      expect(result).toEqual(mockResponse);
    });
  });

  describe("getGithubAPIKey", () => {
    it("should get GitHub API key", async () => {
      const mockToken = "github-token";
      mockedAxios.get.mockResolvedValueOnce({ data: { token: mockToken } });

      const result = await getGithubAPIKey(mockAccessToken);

      expect(mockedAxios.get).toHaveBeenCalledWith(
        "http://auth.example.com/oAuthToken",
        {
          headers: { Authorization: `Bearer ${mockAccessToken}` },
          timeout: 10000,
        },
      );
      expect(result).toBe(mockToken);
    });
  });

  describe("getGithubAvailableScope", () => {
    it("should get GitHub available scopes", async () => {
      const mockScopes = ["repo", "user"];
      const mockResponse = {
        status: 200,
        headers: {
          "x-oauth-scopes": mockScopes.join(", "),
        },
      };

      const mockRequestMethod = vi.fn().mockResolvedValueOnce(mockResponse);
      mockedOctokit.mockImplementation(
        () =>
          ({
            request: mockRequestMethod,
            // biome-ignore lint/suspicious/noExplicitAny: This is a mock implementation
          }) as any,
      );

      const result = await getGithubAvailableScope(mockAccessToken);

      expect(result).toEqual(mockScopes);
    });

    it("should return no scopes when scopes response header missing", async () => {
      const mockResponse = {
        status: 200,
        headers: {},
      };

      const mockRequestMethod = vi.fn().mockResolvedValueOnce(mockResponse);
      mockedOctokit.mockImplementation(
        () =>
          ({
            request: mockRequestMethod,
            // biome-ignore lint/suspicious/noExplicitAny: This is a mock implementation
          }) as any,
      );

      const result = await getGithubAvailableScope(mockAccessToken);

      expect(result).toEqual([]);
    });

    it("should return no scopes when all response headers missing", async () => {
      const mockResponse = {
        status: 200,
      };

      const mockRequestMethod = vi.fn().mockResolvedValueOnce(mockResponse);
      mockedOctokit.mockImplementation(
        () =>
          ({
            request: mockRequestMethod,
            // biome-ignore lint/suspicious/noExplicitAny: This is a mock implementation
          }) as any,
      );

      const result = await getGithubAvailableScope(mockAccessToken);

      expect(result).toEqual([]);
    });

    it("should return empty array on error", async () => {
      const mockRequestMethod = vi.fn().mockResolvedValueOnce({ status: 400 });
      mockedOctokit.mockImplementation(
        () =>
          ({
            request: mockRequestMethod,
            // biome-ignore lint/suspicious/noExplicitAny: This is a mock implementation
          }) as any,
      );

      const result = await getGithubAvailableScope(mockAccessToken);

      expect(result).toEqual([]);
    });
  });

  describe("getCodemodDownloadURI", () => {
    const mockName = "test-codemod";
    const mockResponse: CodemodDownloadLinkResponse = {
      link: "http://example.com/codemods/test-codemod",
      version: "1.0.0",
    };

    it("should get download URI with name and token", async () => {
      mockedAxios.get.mockResolvedValueOnce({ data: mockResponse });

      const result = await getCodemodDownloadURI(mockName, mockAccessToken);

      expect(mockedAxios.get).toHaveBeenCalledWith(
        `http://api.example.com/codemods/downloadLink?name=${mockName}`,
        {
          headers: { Authorization: `Bearer ${mockAccessToken}` },
          timeout: 10000,
        },
      );
      expect(result).toEqual(mockResponse);
    });

    it("should get download URI without token", async () => {
      mockedAxios.get.mockResolvedValueOnce({ data: mockResponse });

      const result = await getCodemodDownloadURI(mockName);

      expect(mockedAxios.get).toHaveBeenCalledWith(
        `http://api.example.com/codemods/downloadLink?name=${mockName}`,
        {
          headers: {},
          timeout: 10000,
        },
      );
      expect(result).toEqual(mockResponse);
    });
  });

  describe("getCodemodList", () => {
    const mockResponse: CodemodListResponse = [
      {
        name: "test-codemod",
        slug: "test-codemod",
        arguments: [],
        author: "test-author",
        engine: "jscodeshift",
        tags: [],
        verified: false,
        updatedAt: new Date(),
      },
    ];

    it("should get codemod list with all options", async () => {
      mockedAxios.get.mockResolvedValueOnce({ data: mockResponse });

      const result = await getCodemodList({
        token: mockAccessToken,
        search: "test",
        mine: true,
        all: true,
      });

      expect(mockedAxios.get).toHaveBeenCalledWith(
        "http://api.example.com/codemods/list?search=test&mine=true&all=true",
        {
          headers: { Authorization: `Bearer ${mockAccessToken}` },
          timeout: 10000,
        },
      );
      expect(result).toEqual(mockResponse);
    });

    it("should get codemod list without options", async () => {
      mockedAxios.get.mockResolvedValueOnce({ data: mockResponse });

      const result = await getCodemodList();

      expect(mockedAxios.get).toHaveBeenCalledWith(
        "http://api.example.com/codemods/list",
        {
          headers: {},
          timeout: 10000,
        },
      );
      expect(result).toEqual(mockResponse);
    });
  });

  describe("generateUserLoginIntent", () => {
    it("should generate user login intent", async () => {
      const mockResponse = { id: "session-id", iv: "iv-string" };
      mockedAxios.post.mockResolvedValueOnce({ data: mockResponse });

      const result = await generateUserLoginIntent();

      expect(mockedAxios.post).toHaveBeenCalledWith(
        "http://auth.example.com/intents",
        {},
      );
      expect(result).toEqual(mockResponse);
    });
  });

  describe("confirmUserLoggedIn", () => {
    it("should confirm user logged in", async () => {
      const mockSessionId = "session-id";
      const mockIv = "iv-string";
      const mockToken = "confirmed-token";
      mockedAxios.get.mockResolvedValueOnce({ data: { token: mockToken } });

      const result = await confirmUserLoggedIn(mockSessionId, mockIv);

      expect(mockedAxios.get).toHaveBeenCalledWith(
        `http://auth.example.com/intents/${mockSessionId}?iv=${mockIv}`,
      );
      expect(result).toBe(mockToken);
    });
  });

  describe("createCodeDiff", () => {
    it("should create code diff", async () => {
      const mockRequest = {
        beforeSnippet: "old code",
        afterSnippet: "new code",
      };
      const mockResponse = { id: "diff-id", iv: "iv-string" };
      mockedAxios.post.mockResolvedValueOnce({ data: mockResponse });

      const result = await createCodeDiff(mockRequest);

      expect(mockedAxios.post).toHaveBeenCalledWith(
        "http://api.example.com/diffs",
        {
          before: mockRequest.beforeSnippet,
          after: mockRequest.afterSnippet,
          source: "cli",
        },
      );
      expect(result).toEqual(mockResponse);
    });
  });
});
