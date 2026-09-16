this is the demo i want to present on friday.

"""
so this is a prototype of a new workflow orchestration engine for codemod.
this is not a complete replacement for butterflow. this reuses a lot of its infrastructure but provides a typescript first authoring experience instead of the current yaml syntax.

here's how it works. i have a `01-shell.ts` file in the folder where `export default shell({...})` creates a shell step. A bare root step is run once. when i go to my terminal and run `codemod-workflow shell.ts` it runs that shell command and returns the result.

i also have a `02-jssg.ts` file that creates a jssg step and returns it. it describes the language, which files should be included and excluded, and the transform function that receives the program root. it just returns some data. i run `codemod-workflow jssg.ts` and it runs that step across the codebase and reports the results that came from each file.

`03-sequence.ts` here i have multiple jssg steps defined, one of which is imported from some shared library. there's also a shell step defined here. i can use `sequence` to, well, run them in a sequence. each step performs some transformation one after the other, and finally the shell command runs to perform some verification. (come up with an example)
i have another example here where the jssg steps receive some input and output, which they can define here through a schema.
now putting them in a sequence means the output of one step passes as input into another. the first step requires a config string of some sort which we will need to pass in when we run this sequence. it outputs this data that is then required input into the second step and so on. so if we want to build steps that are meant to go together we have them share types and schemas and make the data flow very explicit.

`04-parallel.ts` shows how you can define stuff that runs in parallel. so here i am running 3 jssg steps in parallel. all three of these are read only analysis steps, they don't rely on each other and are compeltely independent, so you can easily have them run in parallel with each other, no need for one of them to wait for the other. this also means we can't take the output of one step and input it into another, so this is meant for things that are independent or don't overlap.

i can put these together as well to construct a workflow.
in `05-composed.ts` i have a sequence of steps. starts with a shell step, then 3 parallel jssg steps, then a jssg transform, then parallel shell steps. if you look at this piece of code you know excactly whats going on inside the workflow, you are not building this abstract DAG that will be scheduled dynamically.

that said, you definitely want the ability to do dynamic branching and scheduling and stuff, in which case `06-dynamic.ts` shows `dynamic` step. this receives a typescript function that can call the shell step, decides if it needs migration and then runs the migration step, then returns the project data.

we can also directly call a sequence inside dynamic, which is the same as calling the first function, storing the result in a variable, then calling the next one, storing the result, and so on. it's like a pipe function, it's a convenient syntax over a very common use case.
we can also use parallel inside dynamic to have steps running in parallel.
we can also have dynamic steps called inside parallel or sequence workflows.
so you can see how all of these primitives simply compose on top of each other giving you full flexibility in how you want to build your workflow.

for the simplest cases you can just export a single jssg function and not think about anything else.

you can use sequence and parallel to construct a static plan for a workflow that can be executed by the workflow engine directly, and you can add dynamic flows wherever you want some dynamism.

so how does this actually work under the hood?
* switch to `current.png`

the workflow step starts with the CLI, and the very first thing it does is invoke the bundle splitter.
the bundle splitter receives the workflow typescript file and splits it into 3 bundles - plan, dynamic, and jssg.

the transform functions provided in the jssg steps are extracted into the jssg bundle,
the dynamic typescript functions are extracted into the dynamic bundle,
and the remaining workflow file goes into the plan bundle with the jssg and dynamic functions replaced with stubs that point to where the function lives in the other bundles.

then the plan builder runs the plan bundle and creates the plan for the workflow. this basically runs this workflow file and collects the default export workflow structure that needs to execute. the plan is kindof like an IR that represents the workflow. this is kindof equivalent to the yaml workflow definition we have today, but you don't author this by hand, you write typescript code that runs in the plan builder stage to create this plan.

then the plan is handed off to an orchestrator that actually goes through the plan and sends commands to the underlying rust engine to execute commands in the filesystem. the orchestrator also uses a dynamic runner to run the dynamic workflows when needed.
the underlying rust engine spins up sandboxes for running the jssg code as usual.

this is how its implemented today, and the main reason everything is in node is because i am more familiar with typescript than with rust, so it was easier for me to build this prototype.

the ideal future version would look like `future.png` where the plan builder and dynamic runner have been replaced with similar quickjs sandboxes like with jssg, and everything else that currently runs in node can be migrated to rust.
"""
