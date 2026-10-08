use super::Ctx;
use super::args::CodeProviderArgs;
use crate::code::Provider;

pub fn provider(ctx: &Ctx, args: &CodeProviderArgs) -> Provider {
    Provider {
        program: args.provider.as_ref().map(|p| {
            if p.is_absolute() || p.components().count() == 1 {
                p.clone()
            } else {
                ctx.env.cwd.join(p)
            }
        }),
        args: args.provider_args.clone(),
        files: args
            .provider_files
            .iter()
            .map(|p| ctx.env.cwd.join(p))
            .collect(),
        timeout_seconds: args.provider_timeout,
    }
}
