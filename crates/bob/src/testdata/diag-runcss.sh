#!/bin/bash

SCRIPT_DIR="$( cd "$( dirname "${BASH_SOURCE[0]}" )" && pwd )"
TOP=${SCRIPT_DIR}/

source ${TOP}/opi-versions

# The location of css.sh
CSS_RUN_SCRIPT=$(configure-ioc s -p CSS-gui)

# pop the opifile off the head of the argument list
opifile="${1:-shared/overview.opi}"
shift

function usage() {
    printf "\\nUsage: $0 opi-file -m macro1=value1,macro2=value2\\n\\n"
    printf "       opi-file path should be relative to $(basename "$0")\\n\\n"
    printf "       Additional arguments are passed to css.sh.\\n"
    printf "       See $CSS_RUN_SCRIPT -h for more.\\n\\n"
}

# Check for valid arguments.
if [[ -z $opifile || $opifile == "-h" || $opifile == "--help" ]]; then
    usage
    exit 1
fi
# Check that requested opi file exists.
if [[ ! -f $SCRIPT_DIR/$opifile ]]; then
    echo "OPI file $SCRIPT_DIR/$opifile does not exist"
    usage
    exit 1
fi

if [ -e "${TOP}/configure/VERSION" ] ; then
    version="$(cat "${TOP}"/configure/VERSION)"
else
    version="dev"
fi
project=diagOpi_${version}
module=diagOpi
launch_opi=/${project}/${module}/$opifile


make_link_app()
{
    echo "$1/${2}App/opi/opi=/$project/$2"
}

make_link_opi()
{
    echo "$1/opi=/$project/$2"
}


links="$(join , \
    $TOP=/$project/$module \
    $(make_link_app $MOTOR motor) \
    $(make_link_app $DEVIOCSTATS devIocStats) \
    $(make_link_app $ENZLOCUM4 enzLoCuM4) \
    $(make_link_app $PMACUTIL pmacUtil) \
    $(make_link_app $TIMINGTEMPLATES TimingTemplates) \
    $(make_link_app $_4CHTIMER 4chTimer) \
    $(make_link_app $DLSPLC dlsPLC) \
    $(make_link_opi $LIBERA Libera) \
    $(make_link_opi $PBPM IsaPBPM) \
    $(make_link_opi $MBF MBF) \
)"

$CSS_RUN_SCRIPT -o "${launch_opi}" -s -l "$links" "$@"
