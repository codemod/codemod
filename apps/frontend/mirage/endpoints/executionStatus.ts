import type { Result } from "@/types/github";
import { Response, type Server } from "miragejs";
import type { AppRegistry } from "..";
import { EXECUTION_STATUS } from "../../mocks/endpoints/gh-run";

const isSuccess = true;
const errorExecutionResult: Result = {
  status: "error",
  message: "error msg",
};

const buildGetResponse = () => {
  const responses = [
    {
      status: "progress",
      message: "progress msg 1",
    },
    {
      status: "progress",
      message: "progress msg 2",
    },
    {
      status: "executing codemod",
      progress: { processed: 0, total: 100 },
    },
    {
      status: "executing codemod",
      progress: { processed: 30, total: 100 },
    },
    {
      status: "executing codemod",
      progress: { processed: 80, total: 100 },
    },
    isSuccess
      ? {
          status: "done",
          link: "https://www.google.com", // PR Link
        }
      : errorExecutionResult,
  ];

  let index = 0;

  return () => {
    const currentResponse = responses[index];

    index++;

    return currentResponse;
  };
};

const responsesMap = new Map<string, () => Result>();

export const executionStatusEndpoints = (server: Server<AppRegistry>) => {
  server.get(EXECUTION_STATUS, (_, request) => {
    const codemodRunId = request.params.id ?? "";

    if (!responsesMap.has(codemodRunId)) {
      responsesMap.set(codemodRunId, buildGetResponse());
    }

    const getResponse = responsesMap.get(codemodRunId)!;
    const nextResponse = getResponse();

    return new Response(200, {}, { result: nextResponse, success: true });
  });
};
