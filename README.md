# new-repo-template

Use for creation of mise/backlog/cursor repositories. When creating a new repository, selecrt
this one in the drop-down menu for "Start with a template."

# Initializing the tools

## init [mise](https://mise.jdx.dev/)

install all configured tools

``` bash
mise install
```

## init [backlog](https://github.com/MrLesk/Backlog.md)

``` bash
backlog init "REPLACE WITH PROJECT NAME"
```


# MACOS prerequisites

## Setup git

``` bash
git config --global user.name "John Doe"
```

``` bash
git config --global user.email johndoe@a3analytics.ai
```

## Show all hidden files by default

``` bash
defaults write com.apple.finder AppleShowAllFiles YES
```

``` bash
killall Finder
```

## Install [Homebrew](https://brew.sh/)


``` bash
/bin/bash -c "$(curl -fsSL https://raw.githubusercontent.com/Homebrew/install/HEAD/install.sh)"
```

## Install [mise](https://mise.jdx.dev/)

``` bash
brew install mise
```

### [Activate mise](https://mise.jdx.dev/getting-started.html#activate-mise)

``` zsh
echo 'eval "$(mise activate zsh)"' >> ~/.zshrc
```

To ensure everything is correctly configured with mise.

``` bash
mise dr
```

## Optional [oh-my-zsh](https://ohmyz.sh/)

The simplest and cleanes theme is "simple", to edit open ~/.zshrc and update ZSH_THEME to be "simple"
A useful plugin is "z", edit ~/.zshrc and update plugins=(git z)

``` bash
sh -c "$(curl -fsSL https://raw.githubusercontent.com/ohmyzsh/ohmyzsh/master/tools/install.sh)"
```