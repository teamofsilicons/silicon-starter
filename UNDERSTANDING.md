Silicon Starter is a place where people can pull predefined silicon architectures and use it as a startinging point, or right away as their silicon.

it exists so that us, and people can publish the silicons we've built for others to use and try. different one for different purposes. Others can then pull those starters as their starting point. they can edit if they want to, but if not... they can just use it as is. It should be version controlled, and should auto update on changes.

It has the following capabilities:
- downloading starters (starter name - orgid.starterid)
- search / browse (semantic, embedding the files uploaded)
- release a new version
- discussions (reverse chronological. reddit style threads. maintainer updates. release updates.)
- fork
- see stats (number of published versions, updates, downloads, stars)
- stars
- public / private (to download private, one must be logged into the account, otherwise login is optional)
- silicon.yaml view

silicon.yaml must be in the base dir. it can otherwise contain folders (inc. empty via .siliconkeep) and files. .siliconkeep is automatically added to empty dir


its a CLI first interface. everything should be possible via the cli.
once logged in from IAM (as carbon or silicon), one should be able to work on any starter they are a part of. by default all starts are public, and all private ones have org wide access.
then, they can create a folder, work inside it, make their starter and push it. each commited version stays on the system. they can publish a version and give it a number (XX.XX).

others can then download any public or private starter they have acces to.
by default it has auto updates on. but they can stop it, and pin it to a specific version.
updates try to merge programatically first. if it can't happen cleanly (across all files), then it uses silicon-omni to merge (general model).

we never leave a merge half done. we never show merge conflict. its either merged, or its now.

inside discussions, people can leave comments to a starter. others can reply, to reply to replies. its like reddit forum. this forum also has update blocks when something new is released along with release notes (expandable). carbons/silicons of the org get a special tag.

search is semantic. we embed each latest released version of the starter to be able to search through it semantically.

silicon.yaml gets a unique view where people can see all the blocks of this silicon. this is the deinition of a silicon. right now that means each block is made to look pretty & color coded.

# starter
a starter always looks like this
<name>
|- silicon.yaml
|- anything/
|- else.txt

all it needs is silicon.yaml file in the base dir.

# silicon.yaml
check silicon stemcell for what silicon.yaml is, how it works.

## internals
cli use git internally to manage the starter. there is only one branch (main) and a fork is a seperate linear main branch.

for embedding, we'll get the embeddings from gemini-embedding-2 and retrival is a search query.
for all metadata, vector space & others, lets use postgres.

the backend is a rust backend. and the CLI is built on top of a stateless Rust Package. Then, we use that rust package to run a deamon + cli interface (`starter ...`). the deamon also checks for updates of itself & any starter with auto updates once every hour. and updates it if needed.

for merges that fail to merge... use silicon-omni and ask the best from all available providers to fix the merge conflict. run omni one by one (not in parallel). and wait for silicon to go idle before updating.

frontend is in solidjs.

### starter cli
`starter init` starts a new starter, sets up the main branch in git. seed a .gitignore
`starter new {public/private} orgid.starterid` create a new empty starter.
`starter commit "msg"` commit uncommited changes (add all except ones in .gitignore)
`starter push orgid.starterid` req. only first time.
`starter push` for other times when this git repo has already been pushed to a starter.
`starter pull` or `starter pull orgid.starterid` first time (access required)

- Publish:
`starter publish {latest/commit hash first N (def. 5) unique or complete} Y.X` publishes a new version (combined Y.X should always go up)
`starter publish history` shows the history of all publishes

- Download
`starter download orgid.starterid` (annonymous allowed for public starters, login for private else 404)
`starter download orgid.starterid@Y.X` to get a specific release and turn off auto updates
`starter download orgid.starterid@commithash` to download a specific commit and turn off updates

- Other usage
`starter update off` turns off updates when run from inside any starter
`starter update on` turns updates on
`starter update now` checks & updates now if available

`starter publish history` shows the history
`starter commit history` shows the history

`starter revert {Y.X}` to go to Y.X
`starter revert {commithash}` to go to a commit hash
revert makes a new commit. never rewrite history.
revert goes through merge first. then invokes omni

- Discussions, search, stars, & fork are all possible via the CLI.
Everything possible, by a carbon or a silicon can be done via the CLI.
Web is a subset of all possible things and primarily caters to carbons.

# all auth should happen via IAM. on both CLI and web.

# updates
all auto updates (via download) are not pushable (even if carbon/silicon has permissions). maintain a local version history before merging & updating. each update becomes a local commit.

even if local has changes, attempt to update it. commit before updating. & invoke omni if programatic merge fails.

this is not codebase that will fail upon merge, and no one is looking at merges that closely. each download is more like an app on the phone. people will mostly just download and use it. updates are like updates to the app.

NEVER AUTO UPDATE dev starts. i.e. ones that are pulled, and allows pushes.

# pull vs download
downloads are auto-updated. downloads should not be pushable.
pull are never auto-updated. they are pushable.

# git
all git operations are run locally on the device that is downloading/pulling the starter. no diff, merge etc happens on briefcase. Briefcase is just for storage.

# storage
starter uses briefcase to store all the starters. briefcase allows making things public. store the contents of .git inside briefcase and use that recreate the version history and latest. store it in public folder which by default is only open to all people in the org. so give it allow access to all to ensure it can be used publically by anyone on the internet.

# web
very similar to github.

# auth
all auth is managed by IAM.
add a webhook for when something changes in iam. use permissions and logins from iam.⠐and ask for all orgs this person⠄is a part of. so on the left hald side show a small sidebar like⡀in slack which shows all orgs. make a + button which takes the user to the auth screen to allow/attach one⠂or more orgs than what the user already has.
app name: tos>starter (make an app if not already, iam cli is installed and loggin in)

for other iam apps like briefcase that you rely upon, get its scopes in⠄as well.

# templates
starters are essentially templates that can be seeded into functioning silicons.
for this inside .starterbase/ folder is used to store all the things, raw materials, scripts and state.
this is idempotent. and outputs the final silicon in the parent dir where silicon.yaml will be living.
.starterbase/starter.yaml has all the configurations needed along with variables.

everything needed to build this is inside ./starter_template/

this should be run on its own when ran `starter pull ...` for the first time.
or when ran `starter seed` should rerun this.

if there is a merge conflict during auto updates that can not be resolved on it own, then dont merge that and turn off auto updates.

when pushing the starter, do a compilation check to see if there are any problems with .starterbase

btw, .starterbase is not required
it may not even exist, and in that case it should just use whatever there is. this is for starters that want to give a configuration.

when uploading, run the seed once with all the defaults. this is what carbons & silicons will see and judge when they land on the starter. this also fulfills the req. that there should be a silicon.yaml when pushing.

on the website we should also show all the questions it has, flow of it. and .starterbase for anyone who wants to see what all can they do in this starter.



# testing
IAM supports making a testing env so starter can be passed a app secret at times which means its in testing.

# deployment
aws for backend & storage, crates for rust packages, github for codebase, vercel for frontend.
frontend on `starter.teamofsilicons.com` and backend `backend.starter.teamofsilicons.com`
namecheap cli is also installed along with aws cli, crates creds (shubham/unlikefraction/tos/silicon-omni/.keys), gh cli & vercel cli.

dont use docker in production. for backend, bundle the rust and deploy it directly on an ec2 instance. similarly for anything else we can do without requiring docker. its easy to attach things to systemd, maintain our own logs and manually deploy when a change is made.

# make it prod worthy with upto 100 pushes daily, and about 10k pulls+downloads / day

codebase should be modularised. ship binaries, never raw codebases.