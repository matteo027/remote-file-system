import { Entity, Column, PrimaryColumn, ManyToOne, JoinColumn, OneToMany } from "typeorm";
import { User } from "./User";
import { Group } from "./Group";
import { Path } from "./Path";

@Entity()
export class File {
  @PrimaryColumn()
  ino: string;

  @OneToMany(() => Path, (path) => path.file)
  paths: Path[];

  @ManyToOne(() => User, (user) => user.files)
  @JoinColumn({ name: "owner", referencedColumnName: "uid" })
  owner: User;

  @Column({nullable:false})
  type: number; // 0 = file, 1 = directory, 2 = symlink, etc.

  @Column({nullable:false})
  permissions: number;

  @ManyToOne(() => Group, (group) => group.files, { nullable: true })
  @JoinColumn({ name: "group", referencedColumnName: "gid" })
  group: Group;

}