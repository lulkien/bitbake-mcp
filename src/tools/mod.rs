//! The sixteen tools: the server type, its tool declarations, and the shared
//! "resolve cwd, run, serialize" path every tool ends in.

mod argv;
mod envelope;
mod handler;
mod requests;

use crate::{config, digest, exec, validation};
use envelope::validation_error;
use requests::{
    BitbakeRecipeReq, BuildDirReq, DeployTargetReq, DevtoolAddReq, DevtoolBuildImageReq,
    DevtoolBuildReq, DevtoolCheckUpgradeStatusReq, DevtoolFinishReq, DevtoolModifyReq,
    DevtoolRecipeReq, DevtoolResetReq, DevtoolSearchReq, DevtoolUndeployTargetReq,
    DevtoolUpdateRecipeReq, DevtoolUpgradeReq,
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
    async fn run_cmd(&self, program: &str, args: Vec<String>, build_dir: Option<String>) -> String {
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
    /// Add a recipe from a fetch URI into the workspace.
    #[tool(
        description = "Run 'devtool add [-V version] [-S rev | -a] [-B branch] <recipe> <uri>'. Writes only to the workspace layer and workspace/sources/.",
        annotations(destructive_hint = false, idempotent_hint = false, open_world_hint = true)
    )]
    async fn devtool_add(
        &self,
        Parameters(DevtoolAddReq {
            recipe,
            fetch_uri,
            version,
            srcrev,
            autorev,
            srcbranch,
            build_dir,
        }): Parameters<DevtoolAddReq>,
    ) -> String {
        if let Err(v) = validation::validate_fetch_uri(&fetch_uri) {
            return validation_error(&v);
        }
        if let Some(value) = version.as_deref() {
            if let Err(v) = validation::validate_version(value) {
                return validation_error(&v);
            }
        }
        if let Some(value) = srcrev.as_deref() {
            if let Err(v) = validation::validate_srcrev(value) {
                return validation_error(&v);
            }
        }
        if let Some(value) = srcbranch.as_deref() {
            if let Err(v) = validation::validate_branch(value) {
                return validation_error(&v);
            }
        }
        if srcrev.is_some() && autorev.unwrap_or(false) {
            return validation_error(&validation::ValidationError {
                error_code: "INVALID_ARGUMENT",
                message: "srcrev and autorev are mutually exclusive".into(),
            });
        }
        if let Err(v) = validation::validate_recipe_name(&recipe) {
            return validation_error(&v);
        }
        let mut args = argv::devtool_add(version, srcrev, autorev, srcbranch);
        args.extend([recipe, fetch_uri]);
        self.run_cmd("devtool", args, build_dir).await
    }

    /// Upgrade a workspace recipe in place.
    #[tool(
        description = "Run 'devtool upgrade [-V version] [-S rev] [-B branch] [--no-patch] <recipe>'. Writes only to the workspace layer and workspace/sources/.",
        annotations(destructive_hint = false, idempotent_hint = false, open_world_hint = true)
    )]
    async fn devtool_upgrade(
        &self,
        Parameters(DevtoolUpgradeReq {
            recipe,
            version,
            srcrev,
            srcbranch,
            no_patch,
            build_dir,
        }): Parameters<DevtoolUpgradeReq>,
    ) -> String {
        if let Some(value) = version.as_deref() {
            if let Err(v) = validation::validate_version(value) {
                return validation_error(&v);
            }
        }
        if let Some(value) = srcrev.as_deref() {
            if let Err(v) = validation::validate_srcrev(value) {
                return validation_error(&v);
            }
        }
        if let Some(value) = srcbranch.as_deref() {
            if let Err(v) = validation::validate_branch(value) {
                return validation_error(&v);
            }
        }
        if let Err(v) = validation::validate_recipe_name(&recipe) {
            return validation_error(&v);
        }
        let mut args = argv::devtool_upgrade(version, srcrev, srcbranch, no_patch);
        args.push(recipe);
        self.run_cmd("devtool", args, build_dir).await
    }

    /// Write workspace changes back to the original recipe layer or append layer.
    #[tool(
        description = "Run 'devtool update-recipe [-m mode] [-a layer] [-N] [-n] <recipe>'. Writes committed workspace changes to the original recipe's layer, or a bbappend in append; run with dry_run=true first to preview changes. Destructive: rewrites recipe files and may remove obsolete patches.",
        annotations(destructive_hint = true, idempotent_hint = false, open_world_hint = false)
    )]
    async fn devtool_update_recipe(
        &self,
        Parameters(DevtoolUpdateRecipeReq {
            recipe,
            mode,
            append,
            dry_run,
            no_remove,
            build_dir,
        }): Parameters<DevtoolUpdateRecipeReq>,
    ) -> String {
        if let Err(v) = validation::validate_recipe_name(&recipe) {
            return validation_error(&v);
        }
        let mut args = match argv::devtool_update_recipe(mode, append, dry_run, no_remove) {
            Ok(args) => args,
            Err(v) => return validation_error(&v),
        };
        args.push(recipe);
        self.run_cmd("devtool", args, build_dir).await
    }

    /// Move a workspace recipe into a layer and remove its workspace entry.
    #[tool(
        description = "Run 'devtool finish [-m mode] [-N] [-n] <recipe> <destination>'. Writes the recipe changes into destination and removes the recipe from the workspace; run with dry_run=true first to preview changes. Destructive: writes to the destination layer and removes the workspace entry.",
        annotations(destructive_hint = true, idempotent_hint = false, open_world_hint = false)
    )]
    async fn devtool_finish(
        &self,
        Parameters(DevtoolFinishReq {
            recipe,
            destination,
            mode,
            dry_run,
            no_clean,
            build_dir,
        }): Parameters<DevtoolFinishReq>,
    ) -> String {
        if let Err(v) = validation::validate_recipe_name(&recipe) {
            return validation_error(&v);
        }
        if let Err(v) = validation::validate_layer_dest(&destination) {
            return validation_error(&v);
        }
        let mut args = argv::devtool_finish(mode, dry_run, no_clean);
        args.extend([recipe, destination]);
        self.run_cmd("devtool", args, build_dir).await
    }

    /// Search recipes by name or summary.
    #[tool(
        description = "Run 'devtool search <keyword>' to find recipes by name or summary. keyword must be printable ASCII without whitespace or a leading hyphen. Read-only.",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn devtool_search(
        &self,
        Parameters(DevtoolSearchReq { keyword, build_dir }): Parameters<DevtoolSearchReq>,
    ) -> String {
        if let Err(v) = validation::validate_search_keyword(&keyword) {
            return validation_error(&v);
        }
        self.run_cmd("devtool", argv::devtool_search(keyword), build_dir)
            .await
    }

    /// Find the recipe file for a recipe name.
    #[tool(
        description = "Run 'devtool find-recipe <recipe>' to print the recipe file path. This command is read-only.",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn devtool_find_recipe(
        &self,
        Parameters(DevtoolRecipeReq { recipe, build_dir }): Parameters<DevtoolRecipeReq>,
    ) -> String {
        self.run_tool_cmd("devtool", argv::devtool_find_recipe(), recipe, build_dir)
            .await
    }

    /// Check upstream version status for selected recipes or all recipes.
    #[tool(
        description = "Run 'devtool check-upgrade-status [<recipe>...]' or 'devtool check-upgrade-status -a'. Provide 1 to 32 recipe names, or all=true to check every recipe. Read-only.",
        annotations(read_only_hint = true, open_world_hint = true)
    )]
    async fn devtool_check_upgrade_status(
        &self,
        Parameters(DevtoolCheckUpgradeStatusReq {
            recipes,
            all,
            build_dir,
        }): Parameters<DevtoolCheckUpgradeStatusReq>,
    ) -> String {
        if let Err(v) = validation::validate_upgrade_status_selector(recipes.as_deref(), all) {
            return validation_error(&v);
        }
        let args = argv::devtool_check_upgrade_status(recipes, all);
        self.run_cmd("devtool", args, build_dir).await
    }

    /// Print configure options without invoking a pager.
    #[tool(
        description = "Run 'devtool configure-help -p <recipe>' to print configure options for a workspace recipe. The -p flag always disables the pager.",
        annotations(destructive_hint = false, idempotent_hint = true, open_world_hint = true)
    )]
    async fn devtool_configure_help(
        &self,
        Parameters(DevtoolRecipeReq { recipe, build_dir }): Parameters<DevtoolRecipeReq>,
    ) -> String {
        self.run_tool_cmd("devtool", argv::devtool_configure_help(), recipe, build_dir)
            .await
    }

    /// Run `devtool modify [--debug-build] <recipe>` in the build directory.
    #[tool(
        description = "Run 'devtool modify <recipe>' in the build directory to start modifying a recipe. Set debug_build=true to pass --debug-build and enable DEBUG_BUILD=1 in the workspace append for on-target debugging.",
        annotations(destructive_hint = false, idempotent_hint = false, open_world_hint = true)
    )]
    async fn devtool_modify(
        &self,
        Parameters(DevtoolModifyReq {
            recipe,
            debug_build,
            build_dir,
        }): Parameters<DevtoolModifyReq>,
    ) -> String {
        let argv = argv::devtool_modify(debug_build);
        self.run_tool_cmd("devtool", argv, recipe, build_dir).await
    }

    /// Run `devtool build [-c] [-s] <recipe>` in the build directory.
    #[tool(
        description = "Run 'devtool build <recipe>' in the build directory to build a recipe. Set clean=true to pass -c (clean up the recipe's build results first); set disable_parallel_make=true to pass -s and disable parallel make. Destructive: clean=true removes the recipe's build results.",
        annotations(destructive_hint = true, idempotent_hint = false, open_world_hint = true)
    )]
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
    #[tool(
        description = "Run 'devtool build-image [-p <packages>] <image>' in the build directory. image is required; add_packages is a comma-separated list of recipe names passed to -p.",
        annotations(destructive_hint = false, idempotent_hint = false, open_world_hint = true)
    )]
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
    #[tool(
        description = "Run 'devtool status' in the build directory to list the recipes currently in the workspace — i.e. those set up for modification with devtool modify — each with its external source tree and recipe file. Takes no other arguments. Read-only.",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn devtool_status(
        &self,
        Parameters(BuildDirReq { build_dir }): Parameters<BuildDirReq>,
    ) -> String {
        // Command: devtool status
        self.run_cmd("devtool", vec!["status".to_string()], build_dir)
            .await
    }

    /// Run `devtool reset [-n] <recipe>` in the build directory.
    #[tool(
        description = "Run 'devtool reset <recipe>' to undo a devtool modify. Passes -n unless no_clean=false, i.e. the recipe's sysroot output is not cleaned by default. Destructive: removes the recipe from the workspace and can clean its sysroot output.",
        annotations(destructive_hint = true, idempotent_hint = true, open_world_hint = false)
    )]
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
    #[tool(
        description = "Run 'bitbake [-f] [-c <task>] [-C <task>] [--runall <task>] [--runonly <task>] [--no-setscene|--skip-setscene|--setscene-only] [-k] [-n] [-S none|printdiff] <recipe>' in the build directory. Omit task to run the recipe's default task; task='listtasks' lists available tasks. continue_on_error=true adds -k; dry_run=true adds -n; dump_signatures selects the -S signature diagnostic. Destructive: tasks such as cleansstate or cleanall remove sstate or downloads; use dry_run=true first.",
        annotations(destructive_hint = true, idempotent_hint = false, open_world_hint = true)
    )]
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
    #[tool(
        description = "Run 'devtool deploy-target <recipe> <user@host>' to deploy a recipe to a remote target. Optional flags: strip=true adds -S (default false: --no-strip); no_host_check (-c, default true), dry_run (-n, default false), no_preserve (-p, default true), no_check_space (--no-check-space, default true) each pass their flag unless set to false; port sets -P (default 22). Destructive: overwrites files on the remote target; use dry_run=true first.",
        annotations(destructive_hint = true, idempotent_hint = true, open_world_hint = true)
    )]
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
    #[tool(
        description = "Run 'devtool undeploy-target [-c] [-a] [-n] -P <port> [<recipe>] <user@host>' to remove previously deployed files from a target. Set exactly one of recipe or all=true; all=true removes every recipe previously deployed to that target. no_host_check defaults to true; dry_run=true lists files without removing them; port defaults to 22. Destructive: deletes deployed files from the remote target; use dry_run=true first.",
        annotations(destructive_hint = true, idempotent_hint = true, open_world_hint = true)
    )]
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
