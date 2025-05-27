# implementation notes

## single thread
fx is single-threaded. This is for simplicity:
- It is easier to reason about the system if everything stays in the same thread.
- It is easier to find performance problems and optimize.

Despite that, functions can handle concurrent requests, because side effects are async.

To scale across multiple CPU cores, multiple instances of fx can be deployed and load-balanced across.

Each fx instance is expected to handle one type of trigger events (i.e, being an http server, kafka consumer or cron scheduler). To handle multiple event sources, multiple instances of fx need to be deployed.

## no wasi
Again, for simplicity.

This allows to contain all side effects (from function and host perspective) within fx sdk and have tight control over them.

Another reason is flexibility to experiment.

## hot reloads
The goal is to make it very simple to deploy to fx. Back in php days, deploying to shared hosting could be as simple as copying source files over FTP.

fx matches that experience: each function deployed to fx is defined by wasm file and a yaml configuration file containing side effect handling (e.g., database and rpc bindings).

fx constantly monitors updates to those two files and new version of app is deployed instantly after those files are changed.

## limited support for existing side effect libraries
You can, of course, implement a custom database driver for sqlx to work inside fx. But implementing compatibility layer for existing applications is a low priority for this project.

By avoiding being bound by existing ecosystem and investing into fx sdk, you get more flexibility to explore what's possible (see "no wasi" point).

## sdk

Goal of sdk is to provide all necessary side effects and standard library you would need to build a typical web app.

## testing

fx favors writing blackbox end-to-end tests for functions. Each test should be another rpc method.

## permissions

Functions are not able to have any side effects by default. Any access to outside world is granted by a binding in configuration file. This allows running untrusted code (LLM outputs, plugins written by users, etc.)
