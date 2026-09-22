value=report.txt
case $value in
    *.txt) printf 'text file\n' ;;
    *) printf 'other\n' ;;
esac
