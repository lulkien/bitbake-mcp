//! The eight tools: the server type, its tool declarations, and the shared
//! "resolve cwd, run, serialize" path every tool ends in.

mod argv;
mod envelope;
mod handler;
mod requests;

use crate::{config, digest, exec, validation};
use envelope::validation_error;
use requests::{
    BitbakeRecipeReq, BuildDirReq, DeployTargetReq, DevtoolBuildImageReq, DevtoolBuildReq,
    DevtoolModifyReq, DevtoolResetReq, DevtoolUndeployTargetReq,
};
use rmcp::{
    handler::server::{router::tool::ToolRouter, wrapper::Parameters},
    tool, tool_router,
};

#[derive(Debug, Clone)]
pub(crate) struct BitBakeServer {
    tool_router: ToolRouter<Self>,
    exec_ctx: exec::ExecContext,
    /// Default build directory from config; used when a tool call omits
    /// `build_dir`.
    default_build_dir: String,
}

impl BitBakeServer {
    pub(crate) fn new(cfg: config::Config) -> Self {
        // Absolute, because `validate_build_dir` rejects relative paths: a
        // stdio server's cwd is the client's, so "." is resolved once here.
        let default_build_dir = cfg
            .build_dir
            .map(|p| std::path::absolute(&p).unwrap_or(p))
            .or_else(|| std::env::current_dir().ok())
            .unwrap_or_else(|| ".".into())
            .to_string_lossy()
            .into_owned();
        Self {
            tool_router: Self::tool_router(),
            exec_ctx: exec::ExecContext {
                init_script: cfg.init_script.clone(),
                log_dir: config::default_log_dir(),
            },
            default_build_dir,
        }
    }

    /// Resolve the effective working directory for a build command.
    fn resolve_cwd(&self, build_dir: Option<String>) -> String {
        build_dir.unwrap_or_else(|| self.default_build_dir.clone())
    }
}

impl Default for BitBakeServer {
    fn default() -> Self {
        Self::new(config::Config::default())
    }
}

impl BitBakeServer {
    /// Run `program` with an explicit `args` vector in `build_dir` (default
    /// `.`) and return the result as JSON text.
    ///
    /// The single owner of "resolve cwd, run, serialize": validation and argv
    /// building differ per tool, but where a command runs and how its result is
    /// reported must not.
    async fn run_cmd(
        &self,
        program: &str,
        args: Vec<String>,
        build_dir: Option<String>,
    ) -> String {
        let cwd = self.resolve_cwd(build_dir);
        if let Err(v) = validation::validate_build_dir(&cwd) {
            return validation_error(&v);
        }
        let kind = digest::CommandKind::of(program, &args);
        let res = exec::run(&self.exec_ctx, program, &args, &cwd, kind).await;
        serde_json::to_string(&res).unwrap_or_default()
    }

    /// Validate `recipe` and invoke `program` with `argv` followed by the
    /// recipe, in `build_dir` (default `.`), returning the result as JSON text.
    async fn run_tool_cmd(
        &self,
        program: &str,
        argv: Vec<String>,
        recipe: String,
        build_dir: Option<String>,
    ) -> String {
        if let Err(v) = validation::validate_recipe_name(&recipe) {
            return validation_error(&v);
        }
        let mut args = argv;
        args.push(recipe);
        self.run_cmd(program, args, build_dir).await
    }
}

#[tool_router]
impl BitBakeServer {
    /// Run `devtool modify [--debug-build] <recipe>` in the build directory.
    #[tool(description = "Run 'devtool modify <recipe>' in the build directory to start modifying a recipe. Set debug_build=true to pass --debug-build and enable DEBUG_BUILD=1 in the workspace append for on-target debugging.")]
    async fn devtool_modify(
        &self,
        Parameters(DevtoolModifyReq {
            recipe,
            debug_build,
            build_dir,
        }): Parameters<DevtoolModifyReq>,
    ) -> String {
        let argv = argv::devtool_modify(debug_build);
        self.run_tool_cmd("devtool", argv, recipe, build_dir)
            .await
    }

    /// Run `devtool build [-c] [-s] <recipe>` in the build directory.
    #[tool(description = "Run 'devtool build <recipe>' in the build directory to build a recipe. Set clean=true to pass -c (clean up the recipe's build results first); set disable_parallel_make=true to pass -s and disable parallel make.")]
    async fn devtool_build(
        &self,
        Parameters(DevtoolBuildReq {
            recipe,
            clean,
            disable_parallel_make,
            build_dir,
        }): Parameters<DevtoolBuildReq>,
    ) -> String {
        // Command: devtool build [-c] [-s] <recipe>
        let argv = argv::devtool_build(clean, disable_parallel_make);
        self.run_tool_cmd("devtool", argv, recipe, build_dir).await
    }

    /// Build an image with optional extra workspace packages.
    #[tool(description = "Run 'devtool build-image [-p <packages>] <image>' in the build directory. image is required; add_packages is a comma-separated list of recipe names passed to -p.")]
    async fn devtool_build_image(
        &self,
        Parameters(DevtoolBuildImageReq {
            image,
            add_packages,
            build_dir,
        }): Parameters<DevtoolBuildImageReq>,
    ) -> String {
        let argv = match argv::devtool_build_image(add_packages) {
            Ok(argv) => argv,
            Err(v) => return validation_error(&v),
        };
        self.run_tool_cmd("devtool", argv, image, build_dir).await
    }

    /// Run `devtool status` in the build directory.
    #[tool(description = "Run 'devtool status' in the build directory to list the recipes currently in the workspace — i.e. those set up for modification with devtool modify — each with its external source tree and recipe file. Takes no other arguments.")]
    async fn devtool_status(
        &self,
        Parameters(BuildDirReq { build_dir }): Parameters<BuildDirReq>,
    ) -> String {
        // Command: devtool status
        self.run_cmd("devtool", vec!["status".to_string()], build_dir)
            .await
    }

    /// Run `devtool reset [-n] <recipe>` in the build directory.
    #[tool(description = "Run 'devtool reset <recipe>' to undo a devtool modify. Passes -n unless no_clean=false, i.e. the recipe's sysroot output is not cleaned by default.")]
    async fn devtool_reset(
        &self,
        Parameters(DevtoolResetReq {
            recipe,
            no_clean,
            build_dir,
        }): Parameters<DevtoolResetReq>,
    ) -> String {
        // Command: devtool reset [-n] <recipe>
        let argv = argv::devtool_reset(no_clean);
        self.run_tool_cmd("devtool", argv, recipe, build_dir).await
    }

    /// Run bitbake for a recipe or task in the build directory.
    #[tool(description = "Run 'bitbake [-f] [-c <task>] [-C <task>] [--runall <task>] [--runonly <task>] [--no-setscene|--skip-setscene|--setscene-only] [-k] [-n] [-S none|printdiff] <recipe>' in the build directory. Omit task to run the recipe's default task; task='listtasks' lists available tasks. continue_on_error=true adds -k; dry_run=true adds -n; dump_signatures selects the -S signature diagnostic.")]
    async fn bitbake_recipe(
        &self,
        Parameters(BitbakeRecipeReq {
            recipe,
            task,
            force,
            clear_stamp,
            runall,
            runonly,
            no_setscene,
            skip_setscene,
            setscene_only,
            continue_on_error,
            dry_run,
            dump_signatures,
            build_dir,
        }): Parameters<BitbakeRecipeReq>,
    ) -> String {
        // Command: bitbake [-f] [-c <task>] [-C <task>] [--runall <task>]
        //          [--runonly <task>] [--no-setscene] [--skip-setscene]
        //          [--setscene-only] [-k] [-n] [-S <mode>] <recipe>
        let argv = match argv::bitbake_recipe(
            force,
            task,
            clear_stamp,
            runall,
            runonly,
            no_setscene,
            skip_setscene,
            setscene_only,
            continue_on_error,
            dry_run,
            dump_signatures,
        ) {
            Ok(argv) => argv,
            Err(v) => return validation_error(&v),
        };
        self.run_tool_cmd("bitbake", argv, recipe, build_dir).await
    }

    /// Run `devtool deploy-target` in the build directory.
    #[tool(description = "Run 'devtool deploy-target <recipe> <user@host>' to deploy a recipe to a remote target. Optional flags: strip=true adds -S (default false: --no-strip); no_host_check (-c, default true), dry_run (-n, default false), no_preserve (-p, default true), no_check_space (--no-check-space, default true) each pass their flag unless set to false; port sets -P (default 22).")]
    async fn devtool_deploy_target(
        &self,
        Parameters(DeployTargetReq {
            recipe,
            target,
            strip,
            no_host_check,
            dry_run,
            no_preserve,
            no_check_space,
            port,
            build_dir,
        }): Parameters<DeployTargetReq>,
    ) -> String {
        // Command: devtool deploy-target [-c] [-n] [-p] [--no-check-space]
        //          [-P <port>] [-S|--no-strip] <recipe> <target>
        if let Some(port) = port {
            if let Err(v) = validation::validate_port(port) {
                return validation_error(&v);
            }
        }
        if let Err(v) = validation::validate_recipe_name(&recipe) {
            return validation_error(&v);
        }
        if let Err(v) = validation::validate_target(&target) {
            return validation_error(&v);
        }
        let args = argv::deploy_target(
            recipe,
            target,
            strip,
            no_host_check,
            dry_run,
            no_preserve,
            no_check_space,
            port,
        );
        self.run_cmd("devtool", args, build_dir).await
    }

    /// Remove a recipe's deployed files from a target, or all deployed files.
    #[tool(description = "Run 'devtool undeploy-target [-c] [-a] [-n] -P <port> [<recipe>] <user@host>' to remove previously deployed files from a target. Set exactly one of recipe or all=true; all=true removes every recipe previously deployed to that target. no_host_check defaults to true; dry_run=true lists files without removing them; port defaults to 22.")]
    async fn devtool_undeploy_target(
        &self,
        Parameters(DevtoolUndeployTargetReq {
            recipe,
            all,
            target,
            no_host_check,
            dry_run,
            port,
            build_dir,
        }): Parameters<DevtoolUndeployTargetReq>,
    ) -> String {
        if let Err(v) = validation::validate_undeploy_target_selector(recipe.as_deref(), all) {
            return validation_error(&v);
        }
        if let Some(port) = port {
            if let Err(v) = validation::validate_port(port) {
                return validation_error(&v);
            }
        }
        if let Err(v) = validation::validate_target(&target) {
            return validation_error(&v);
        }
        let args = argv::undeploy_target(recipe, all, target, no_host_check, dry_run, port);
        self.run_cmd("devtool", args, build_dir).await
    }
}
