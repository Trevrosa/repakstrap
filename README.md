# repakstrap

bootstrap [repak-rivals](https://github.com/natimerry/repak-rivals), updating on a new version.

## features

- use it the same as repak
- will return the same exit code repak does
- will update repak for you when updates are found
- only do update checks *at most* once every hour
- ~7ms slower startup when updates are not checked (~1ms on linux).

## usage

use the `-U` flag to force an update check. any other arguments passed will be forwarded to repak.

```bash
# let's run it from scratch.
$ ./repakstrap -V
could not find local version!
errors: tried to run `./dist/repak -V` => The system cannot find the file specified.

starting download
downloading 0.5.7/repak_cli-x86_64-pc-windows-msvc.zip
done!
extracted.

summary: got the latest repak 0.5.7

repak_cli 0.5.7

# we can now run any repak command we like. repakstrap won't check for updates for an hour.
$ ./repakstrap list test.pak
skipped checks, last checked `10s` ago.

mount point: ../../../
version: V8B
version major: FNameBasedCompression
encrypted index: false
encrytion guid: Some(00000000000000000000000000000000)
path hash seed: None
4 file entries

# there's a new update! let's force an update check.
$ ./repakstrap -U -V
found new version 0.5.8

starting download
downloading 0.5.8/repak_cli-x86_64-pc-windows-msvc.zip
done!
extracted.

summary: repak 0.5.7 => 0.5.8

repak_cli 0.5.8
```

## limits

the github api only allows 60 unauthenticated api calls per day, meaning only 60 update checks can be run each day.

if you have a [github api key](https://docs.github.com/en/authentication/keeping-your-account-and-data-secure/managing-your-personal-access-tokens), put it in a `REPAKSTRAP_APIKEY` environment variable and the program will use it, allowing infinite update checks (5,000 requests per hour).
