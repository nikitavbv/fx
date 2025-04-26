# implementation notes

## instances vs functions
in Kubernetes, there are pods and deployments

in fx, there are instances and functions

functions are versioned and are a "template" for instances containing following properties:
- version
- reference to code to run
- permissions/bindings list
- is global
- is system

instances contain following properties:
- unique id
- relevant function definition and version
- code to run
- bound storage/network/rpc

functions also serve role of kubernetes services (i.e. rpc calls are sent not to individual instances, but to function)

before instance is invoked, fx checks if it's version matches the latest version of function. if not - it is "redeployed"

## metrics reporting/logging

once per function invocation and once per logged event an event is pushed to internal message bus

functions can listed on internal message bus to update reporting, forward logs, etc.

to avoid endless loops (i.e., function listening on logs emitting logs, calling themselves in a loop), functions marked as `system` should not be using this infrastructure.

## state of the system

fx engine is storing only what is necessary to run the system and exposes this info via apis in `fx_cloud` namespace.

it is up to applications like `fx-cloud-dashboard` to persist metrics/logs.

## queues

queues are built on host side instead of being deployed as a global service. while they can be just another service that is deployed, it is a core part of the system,
so managing them on host side should simplify everything.
