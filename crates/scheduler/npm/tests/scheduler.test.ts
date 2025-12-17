import crypto from "node:crypto";
import { beforeAll, describe, expect, it } from "vitest";
import {
  type MatrixTaskChanges,
  type RunnableTaskChanges,
  Scheduler,
} from "../pkg/butterflow_scheduler";
import type {
  Node,
  NodeType,
  Task,
  TaskStatus,
  TriggerType,
  Workflow,
  WorkflowRun,
} from "../types";

function createBasicNode(id: string, dependsOn: string[]): Node {
  return {
    id,
    name: `Node ${id}`,
    type: "automatic",
    depends_on: dependsOn,
    runtime: {
      type: "direct",
    },
    steps: [
      {
        name: "Step 1",
        run: `echo 'Running ${id}'`,
      },
    ],
    env: {},
  };
}

function createMatrixNodeValues(
  id: string,
  dependsOn: string[],
  values: Record<string, string>[],
): Node {
  return {
    id,
    name: `Node ${id}`,
    type: "automatic",
    depends_on: dependsOn,
    strategy: {
      type: "matrix",
      values: values,
    },
    runtime: {
      type: "direct",
    },
    steps: [
      {
        name: "Step 1",
        run: `echo 'Running matrix ${id}'`,
      },
    ],
    env: {},
  };
}

function createMatrixNodeFromState(id: string, dependsOn: string[], stateKey: string): Node {
  return {
    id,
    name: `Node ${id}`,
    type: "automatic",
    depends_on: dependsOn,
    strategy: {
      type: "matrix",
      values: null,
      from_state: stateKey,
    },
    runtime: {
      type: "direct",
    },
    steps: [
      {
        name: "Step 1",
        run: `echo 'Running matrix from state ${id}'`,
      },
    ],
    env: {},
  };
}

function createManualNode(
  id: string,
  dependsOn: string[],
  nodeType: NodeType,
  triggerType: TriggerType | null,
): Node {
  return {
    id,
    name: `Node ${id}`,
    type: nodeType,
    depends_on: dependsOn,
    trigger: triggerType ? { type: triggerType } : null,
    runtime: {
      type: "direct",
    },
    steps: [
      {
        name: "Step 1",
        run: `echo 'Running manual ${id}'`,
      },
    ],
    env: {},
  };
}

function createTestWorkflow(nodes: Node[]): Workflow {
  return {
    version: "1",
    templates: [],
    nodes,
  };
}

function createTestRun(workflow: Workflow): WorkflowRun {
  return {
    id: crypto.randomUUID(),
    workflow,
    status: "Running",
    params: {},
    tasks: [], // Assuming tasks aren't needed in the input run object for scheduler functions
    started_at: new Date().toISOString(),
  };
}

function createTask(
  wfRunId: string,
  node_id: string,
  is_master: boolean,
  status: TaskStatus = "Pending",
  master_task_id: string | null = null,
  matrixValues: Record<string, string> | null = null,
): Task {
  return {
    id: crypto.randomUUID(),
    workflow_run_id: wfRunId,
    node_id,
    status,
    is_master,
    master_task_id,
    matrix_values: matrixValues,
    error: null,
    logs: [],
    started_at: null,
  };
}

// --- Test Cases --- //

describe("Scheduler WASM Module", () => {
  let scheduler: Scheduler;

  beforeAll(() => {
    scheduler = new Scheduler();
  });

  // Test calculateInitialTasks
  describe("calculateInitialTasks", () => {
    it("should create tasks for simple workflow", async () => {
      const workflow = createTestWorkflow([
        createBasicNode("node1", []),
        createBasicNode("node2", ["node1"]),
      ]);
      const run = createTestRun(workflow);

      const tasks: Task[] = await scheduler.calculateInitialTasks(run);

      expect(tasks).toHaveLength(2);
      expect(tasks.some((t) => t.node_id === "node1" && !t.is_master)).toBe(true);
      expect(tasks.some((t) => t.node_id === "node2" && !t.is_master)).toBe(true);
      expect(tasks.filter((t) => t.is_master).length).toBe(0);
    });

    it("should create master and child tasks for matrix with values", async () => {
      const matrixValues = [{ k: "v1" }, { k: "v2" }];
      const workflow = createTestWorkflow([
        createBasicNode("node1", []),
        createMatrixNodeValues("node2", ["node1"], matrixValues),
      ]);
      const run = createTestRun(workflow);

      const tasks: Task[] = await scheduler.calculateInitialTasks(run);

      expect(tasks).toHaveLength(4); // 1 node1 + 1 master node2 + 2 children node2
      expect(tasks.some((t) => t.node_id === "node1" && !t.is_master)).toBe(true);
      const masterTask = tasks.find((t) => t.node_id === "node2" && t.is_master);
      expect(masterTask).toBeDefined();

      const childTasks = tasks.filter((t) => t.node_id === "node2" && !t.is_master);
      expect(childTasks).toHaveLength(2);
      expect(childTasks.every((t) => t.master_task_id === masterTask?.id)).toBe(true);
      expect(childTasks.some((t) => deepEqual(t.matrix_values?.k, matrixValues[0].k))).toBe(true);
      expect(childTasks.some((t) => deepEqual(t.matrix_values?.k, matrixValues[1].k))).toBe(true);
    });

    it("should create only master task for matrix from state initially", async () => {
      const workflow = createTestWorkflow([
        createBasicNode("node1", []),
        createMatrixNodeFromState("node2", ["node1"], "my_state_key"),
      ]);
      const run = createTestRun(workflow);

      const tasks: Task[] = await scheduler.calculateInitialTasks(run);

      expect(tasks).toHaveLength(2); // 1 node1 + 1 master node2
      expect(tasks.some((t) => t.node_id === "node1" && !t.is_master)).toBe(true);
      expect(tasks.some((t) => t.node_id === "node2" && t.is_master)).toBe(true);
      expect(tasks.filter((t) => t.node_id === "node2" && !t.is_master).length).toBe(0);
    });
  });

  // Test calculateMatrixTaskChanges
  describe("calculateMatrixTaskChanges", () => {
    it("should create master and child tasks if master does not exist", async () => {
      const workflow = createTestWorkflow([
        createBasicNode("node1", []),
        createMatrixNodeFromState("node2", ["node1"], "items"),
      ]);
      const run = createTestRun(workflow);
      const initialState = { items: [{ id: "a" }, { id: "b" }] };
      const initialTasks = [createTask(run.id, "node1", false)]; // Only node1 task

      const changes: MatrixTaskChanges = await scheduler.calculateMatrixTaskChanges(
        run.id,
        run,
        initialTasks,
        initialState,
      );

      const newTasks = changes.new_tasks as Task[];
      const tasksToMarkWontDo = changes.tasks_to_mark_wont_do as string[];
      const masterTasksToUpdate = changes.master_tasks_to_update as string[];

      expect(newTasks).toHaveLength(3); // 1 master + 2 children
      expect(newTasks.some((t) => t.is_master && t.node_id === "node2")).toBe(true);
      expect(tasksToMarkWontDo).toHaveLength(0);
      expect(masterTasksToUpdate).toHaveLength(1);
      const childTasks = newTasks.filter((t) => !t.is_master);
      expect(childTasks).toHaveLength(2);
      expect(childTasks.some((t) => deepEqual(t.matrix_values, new Map([["id", "a"]])))).toBe(true);
      expect(childTasks.some((t) => deepEqual(t.matrix_values, new Map([["id", "b"]])))).toBe(true);
    });

    it("should add new tasks and mark removed tasks as WontDo", async () => {
      const workflow = createTestWorkflow([
        createBasicNode("node1", []),
        createMatrixNodeFromState("node2", ["node1"], "items"),
      ]);
      const run = createTestRun(workflow);
      const node2Id = "node2";

      const masterTask = createTask(run.id, node2Id, true);
      const taskA = createTask(run.id, node2Id, false, "Pending", masterTask.id, { id: "a" });
      const taskB = createTask(run.id, node2Id, false, "Pending", masterTask.id, { id: "b" });

      const initialTasks = [createTask(run.id, "node1", false), masterTask, taskA, taskB];

      // New state: remove 'b', add 'c'
      const newState = { items: [{ id: "a" }, { id: "c" }] };

      const changes: MatrixTaskChanges = await scheduler.calculateMatrixTaskChanges(
        run.id,
        run,
        initialTasks,
        newState,
      );

      const newTasks = changes.new_tasks as Task[];
      const tasksToMarkWontDo = changes.tasks_to_mark_wont_do as string[];
      const masterTasksToUpdate = changes.master_tasks_to_update as string[];

      // Should create task 'c'
      expect(newTasks).toHaveLength(1);
      expect(deepEqual(newTasks[0].matrix_values, new Map([["id", "c"]]))).toBe(true);
      expect(newTasks[0].master_task_id).toBe(masterTask.id);

      // Should mark task 'b' as WontDo
      expect(tasksToMarkWontDo).toHaveLength(1);
      expect(tasksToMarkWontDo[0]).toBe(taskB.id);

      // Should update the master task
      expect(masterTasksToUpdate).toHaveLength(1);
      expect(masterTasksToUpdate[0]).toBe(masterTask.id);
    });

    it("should handle missing state key gracefully", async () => {
      const workflow = createTestWorkflow([createMatrixNodeFromState("node2", [], "items")]);
      const run = createTestRun(workflow);
      const node2Id = "node2";
      const masterTask = createTask(run.id, node2Id, true);
      const initialTasks = [masterTask];
      const emptyState = {};

      const changes: MatrixTaskChanges = await scheduler.calculateMatrixTaskChanges(
        run.id,
        run,
        initialTasks,
        emptyState,
      );

      const newTasks = changes.new_tasks as Task[];
      const tasksToMarkWontDo = changes.tasks_to_mark_wont_do as string[];
      const masterTasksToUpdate = changes.master_tasks_to_update as string[];

      expect(newTasks).toHaveLength(0);
      expect(tasksToMarkWontDo).toHaveLength(0);
      expect(masterTasksToUpdate).toHaveLength(1); // Still updates master
      expect(masterTasksToUpdate[0]).toBe(masterTask.id);
    });
  });

  // Test findRunnableTasks
  describe("findRunnableTasks", () => {
    it("should find runnable tasks based on simple dependencies", async () => {
      const workflow = createTestWorkflow([
        createBasicNode("node1", []),
        createBasicNode("node2", ["node1"]),
      ]);
      const run = createTestRun(workflow);

      const task1_pending = createTask(run.id, "node1", false, "Pending");
      const task2_pending = createTask(run.id, "node2", false, "Pending");

      // Case 1: node1 pending, node2 pending -> only node1 runnable
      let tasks = [task1_pending, task2_pending];
      let runnable: RunnableTaskChanges = await scheduler.findRunnableTasks(run, tasks);
      let runnableTaskIds = runnable.runnable_tasks as string[];
      expect(runnableTaskIds).toHaveLength(1);
      expect(runnableTaskIds[0]).toBe(task1_pending.id);

      // Case 2: node1 completed, node2 pending -> only node2 runnable
      const task1_completed = {
        ...task1_pending,
        status: "Completed" as TaskStatus,
      };
      tasks = [task1_completed, task2_pending];
      runnable = await scheduler.findRunnableTasks(run, tasks);
      runnableTaskIds = runnable.runnable_tasks as string[];
      expect(runnableTaskIds).toHaveLength(1);
      expect(runnableTaskIds[0]).toBe(task2_pending.id);

      // Case 3: node1 running, node2 pending -> no tasks runnable
      const task1_running = {
        ...task1_pending,
        status: "Running" as TaskStatus,
      };
      tasks = [task1_running, task2_pending];
      runnable = await scheduler.findRunnableTasks(run, tasks);
      runnableTaskIds = runnable.runnable_tasks as string[];
      expect(runnableTaskIds).toHaveLength(0);

      // Case 4: node1 completed, node2 completed -> no tasks runnable
      const task2_completed = {
        ...task2_pending,
        status: "Completed" as TaskStatus,
      };
      tasks = [task1_completed, task2_completed];
      runnable = await scheduler.findRunnableTasks(run, tasks);
      runnableTaskIds = runnable.runnable_tasks as string[];
      expect(runnableTaskIds).toHaveLength(0);
    });

    it("should find runnable tasks based on matrix dependencies", async () => {
      const matrixValues = [{ k: "v1" }, { k: "v2" }];
      const workflow = createTestWorkflow([
        createMatrixNodeValues("node1", [], matrixValues),
        createBasicNode("node2", ["node1"]),
      ]);
      const run = createTestRun(workflow);

      const node1_master = createTask(run.id, "node1", true);
      const node1_child1 = createTask(
        run.id,
        "node1",
        false,
        "Pending",
        node1_master.id,
        matrixValues[0],
      );
      const node1_child2 = createTask(
        run.id,
        "node1",
        false,
        "Pending",
        node1_master.id,
        matrixValues[1],
      );
      const node2_task = createTask(run.id, "node2", false, "Pending");

      // Case 1: All node1 tasks pending -> only node1 children runnable
      let tasks = [node1_master, node1_child1, node1_child2, node2_task];
      let runnable: RunnableTaskChanges = await scheduler.findRunnableTasks(run, tasks);
      let runnableTaskIds = runnable.runnable_tasks as string[];
      expect(runnableTaskIds).toHaveLength(2);
      expect(runnableTaskIds).toContain(node1_child1.id);
      expect(runnableTaskIds).toContain(node1_child2.id);

      // Case 2: One node1 child completed, one pending -> node2 not runnable yet
      const node1_child1_completed = {
        ...node1_child1,
        status: "Completed" as TaskStatus,
      };
      tasks = [node1_master, node1_child1_completed, node1_child2, node2_task];
      runnable = await scheduler.findRunnableTasks(run, tasks);
      runnableTaskIds = runnable.runnable_tasks as string[];
      expect(runnableTaskIds).toHaveLength(1); // Only child2 is runnable
      expect(runnableTaskIds[0]).toBe(node1_child2.id);

      // Case 3: Both node1 children completed -> node2 runnable
      const node1_child2_completed = {
        ...node1_child2,
        status: "Completed" as TaskStatus,
      };
      // Mark master as completed too (in real scenario this happens separately)
      const node1_master_completed = {
        ...node1_master,
        status: "Completed" as TaskStatus,
      };
      tasks = [node1_master_completed, node1_child1_completed, node1_child2_completed, node2_task];
      runnable = await scheduler.findRunnableTasks(run, tasks);
      runnableTaskIds = runnable.runnable_tasks as string[];
      expect(runnableTaskIds).toHaveLength(1);
      expect(runnableTaskIds[0]).toBe(node2_task.id);
    });

    it("should mark tasks with manual trigger as awaiting trigger", async () => {
      const workflow = createTestWorkflow([
        createBasicNode("node1", []),
        createManualNode("node2", ["node1"], "automatic", "manual"),
      ]);
      const run = createTestRun(workflow);

      const task1 = createTask(run.id, "node1", false, "Completed");
      const task2 = createTask(run.id, "node2", false, "Pending");

      const runnable: RunnableTaskChanges = await scheduler.findRunnableTasks(run, [task1, task2]);

      const runnableTaskIds = runnable.runnable_tasks as string[];
      const tasksToAwaitTrigger = runnable.tasks_to_await_trigger as string[];

      // No tasks should be directly runnable
      expect(runnableTaskIds).toHaveLength(0);

      // Verify that task2 is awaiting trigger
      expect(tasksToAwaitTrigger).toHaveLength(1);
      expect(tasksToAwaitTrigger[0]).toBe(task2.id);
    });

    it("should mark tasks with manual node type as awaiting trigger", async () => {
      const workflow = createTestWorkflow([
        createBasicNode("node1", []),
        createManualNode("node2", ["node1"], "manual", null), // Manual Node Type
      ]);
      const run = createTestRun(workflow);

      const task1 = createTask(run.id, "node1", false, "Completed");
      const task2 = createTask(run.id, "node2", false, "Pending");

      const runnable: RunnableTaskChanges = await scheduler.findRunnableTasks(run, [task1, task2]);

      const runnableTaskIds = runnable.runnable_tasks as string[];
      const tasksToAwaitTrigger = runnable.tasks_to_await_trigger as string[];

      // No tasks should be directly runnable
      expect(runnableTaskIds).toHaveLength(0);

      // Verify that task2 is awaiting trigger
      expect(tasksToAwaitTrigger).toHaveLength(1);
      expect(tasksToAwaitTrigger[0]).toBe(task2.id);
    });
  });
});

// Simple deep equality check for objects (needed for matrix values)
function deepEqual(obj1: unknown, obj2: unknown): boolean {
  if (obj1 === obj2) return true;

  if (obj1 === null || obj2 === null || typeof obj1 !== "object" || typeof obj2 !== "object") {
    return false;
  }

  // Type guard for null check already happened
  const obj1NonNull = obj1 as Record<string, unknown>;
  const obj2NonNull = obj2 as Record<string, unknown>;

  const keys1 = Object.keys(obj1NonNull);
  const keys2 = Object.keys(obj2NonNull);

  if (keys1.length !== keys2.length) return false;

  for (const key of keys1) {
    if (!keys2.includes(key) || !deepEqual(obj1NonNull[key], obj2NonNull[key])) {
      return false;
    }
  }

  return true;
}
